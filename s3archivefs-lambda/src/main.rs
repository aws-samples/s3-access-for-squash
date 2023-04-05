mod vmap;
mod output;
use std::collections::HashMap;
use aws_lambda_events::s3::object_lambda::S3ObjectLambdaEvent;
use aws_sdk_s3::Client;
use aws_sdk_s3::types::DateTime;
use aws_smithy_types::date_time::Format;
use aws_smithy_http::byte_stream::{ByteStream, Length};
use serde_json::{json, Value};
use lambda_runtime::{run, service_fn, Error, LambdaEvent};
use aws_endpoint::partition;
use aws_endpoint::partition::endpoint;
use aws_endpoint::{CredentialScope, Partition, PartitionResolver};
use url::Url;
use log::{debug, info, warn};
use rand::distributions::DistString;
use crate::output::{Object, ListBucketResult, ListObjectsResponse, ListResultXml, HeadObjectResponse, HeadObjectHeaders};
use crate::vmap::PrefixVMap;
use s3archivefs::repo;

const EXTRACT_TMP_DIR: &str = "/tmp/s3archivefs_temp_files";

#[allow(dead_code)]
fn get_repo_prefix(repo_path: &str, repo_object: &str) -> Option<String> {
    let split = repo_path.split("/");

    let mut v: Vec<&str> = split.collect();
    if let Some(last) = v.pop() {
        if last == repo_object {
            return Some(v.join("/"));
        }
    }
    None
}

// return search top and search key
fn get_repo_search_top_and_key(search_prefix: &str, virtual_prefix: &str) -> (String, String) {
    if search_prefix.len() < virtual_prefix.len() {
        return ("".to_string(), "".to_string());
    }
    let s = search_prefix.trim_start_matches(virtual_prefix).to_string();
    let search_key = s.clone();
    let split: Vec<&str> = s.split("/").collect();
    (split[0..split.len()-1].join("/").to_string(), search_key)
}

fn filter_result(v: &mut Vec<(String, libc::stat64)>, filter_key: String, max_count: usize, last_end: Option<String>) -> Option<String> {

    // start from last_end, trim head head parts of result vec
    if last_end.is_some() {
        if let Some(index) = v.iter().position(|r| &r.0 == last_end.as_ref().unwrap()) {
            v.drain(0..index+1);
        } else {
            v.clear();
            return None;
        }
    }

    v.retain(|x| x.0.starts_with(&filter_key));

    if v.len() <= max_count {
        return None;
    }

    v.drain(max_count..);
    Some(v.last().unwrap().0.to_owned())
}

#[allow(dead_code)]
fn make_uri(endpoint: &str, account: &str) -> &'static str {
    let mut uri = endpoint.to_string();
    uri.push('-');
    uri.push_str(account);
    uri.push_str(".s3-object-lambda.{region}.amazonaws.com");

    Box::leak(uri.into_boxed_str())
}

struct Env {
    region: String,
    vmap: PrefixVMap,
    cache_dir: String,
    chunk_size: Option<usize>,
    hdmode: repo::HoleDetectMode,
}

async fn get_object_handler(event: LambdaEvent<S3ObjectLambdaEvent>, env: Env) -> Result<Value, Error> {

    let context = event.payload.get_object_context.as_ref().unwrap();
    let output_route = event.payload.get_object_context.as_ref().unwrap().output_route.clone();
    let output_token = event.payload.get_object_context.as_ref().unwrap().output_token.clone();
    let input_s3_url = context.input_s3_url.clone();
    let url = Url::parse(&input_s3_url).unwrap();
    let key = url.path();
    info!("get object request key {:?}", key);
    let key = key.trim_start_matches('/');

    let endpoint = Box::leak(format!("s3-object-lambda.{}.amazonaws.com", env.region).into_boxed_str());
    let resolver = PartitionResolver::new(
        Partition::builder()
            .id("aws")
            .region_regex(r#"^(us|eu|ap|sa|ca|me|af)\-\w+\-\d+$"#)
            .default_endpoint(endpoint::Metadata {
                uri_template: endpoint,
                protocol: endpoint::Protocol::Https,
                signature_versions: endpoint::SignatureVersion::V4,
                credential_scope: CredentialScope::builder()
                    .service("s3-object-lambda")
                    .build(),
            })
            .regionalized(partition::Regionalized::Regionalized)
            .build()
            .expect("valid partition"),
        vec![],
    );

    let shared_config = aws_config::load_from_env().await;
    let s3_config = aws_sdk_s3::config::Builder::from(&shared_config)
        .endpoint_resolver(resolver)
        .build();
    let client = Client::from_conf(s3_config);

    let res = env.vmap.query(key);
    if res.is_none() {
        info!("not found in repo map: {:?}", res);
        let _ = client.write_get_object_response()
                        .request_route(output_route)
                        .request_token(output_token)
                        .status_code(404)
                        .error_code("NotFound")
                        .error_message("ObjectNotFound")
                        .send()
                        .await;
        return Ok(json!({"status_code": 200}))
    }
    let (matched_virtual_prefix, repo_bucket, repo_prefix, repo_object) = res.unwrap();
    let repo_key = format!("{}/{}", repo_prefix, repo_object);
    let cachefiledir = format!("{}/{}/{}", env.cache_dir, repo_bucket, repo_prefix);
    let cachefile = format!("{}/{}", cachefiledir, repo_object);
    tokio::fs::create_dir_all(cachefiledir).await.unwrap();
    let remote = repo::Remote::new(&env.region, &repo_bucket, &repo_key).await;
    debug!("Remote object created");
    let local = repo::Local::new(&cachefile, env.chunk_size, env.hdmode, false, false, Some(remote.clone()), false).await;
    debug!("Local object created");
    let repo = local.clone();
    repo::CONTEXT.with(|c| *c.borrow_mut() = Some(local));
    let (repo_top, key) = get_repo_search_top_and_key(key, &matched_virtual_prefix);
    debug!("repo_top {:?}", repo_top);
    debug!("key {:?}", key);

    tokio::fs::create_dir_all(EXTRACT_TMP_DIR).await.unwrap();
    let tempfile = format!("{}/{}", EXTRACT_TMP_DIR, rand::distributions::Alphanumeric.sample_string(&mut rand::thread_rng(), 16));
    info!("extract {} to {}", key, tempfile);
    let res = repo.extract_one(&key, &tempfile);
    if res.is_err() {
        warn!("extract failed: {:?}", res);
        let _ = client.write_get_object_response()
                        .request_route(output_route)
                        .request_token(output_token)
                        .status_code(404)
                        .error_code("NotFound")
                        .error_message("ObjectNotFound")
                        .send()
                        .await;
        return Ok(json!({"status_code": 200}))
    }
    let filesz = res.unwrap();

    let mut is_range = false;
    let mut offset = 0;
    let mut length = filesz as u64;
    let mut bad_range = false;
    // test if range get
    if let Some(header_range_val) = event.payload.user_request.headers.get("range") {
        debug!("this is a range get {}", header_range_val.to_str().unwrap_or_default());
        is_range = true;
        match http_range::HttpRange::parse(header_range_val.to_str().unwrap_or_default(), filesz as u64) {
            Ok(rngs) => {
                if rngs.len() > 1 {
                    // too many ranges, it's bad
                    bad_range = true;
                    warn!("too many ranges");
                } else {
                    offset = rngs[0].start;
                    length = rngs[0].length;
                }
            },
            Err(e) => {
                bad_range = true;
                warn!("range parse failed: {:?}", e);
            },
        }
    }

    if bad_range {
        let resp = client.write_get_object_response()
                        .request_route(output_route)
                        .request_token(output_token)
                        .status_code(416)
                        .error_code("InvalidRange")
                        .error_message("The requested range is not valid for the request. Try another range.")
                        .send()
                        .await;
        debug!("send 416 to client, result: {:?}", resp);
        return Ok(json!({"status_code": 200}))
    }

    let res = ByteStream::read_from()
                    .path(tempfile)
                    .offset(offset)
                    .length(Length::Exact(length))
                    .build()
                    .await;
    if res.is_err() {
        warn!("failed to open tmp file: {:?}", res);
        let resp = client.write_get_object_response()
                        .request_route(output_route)
                        .request_token(output_token)
                        .status_code(400)
                        .error_code("InternalServerError")
                        .error_message("Internal Server Error")
                        .send()
                        .await;
        debug!("send 400 to client, result: {:?}", resp);
        return Ok(json!({"status_code": 200}))
    }
    let bytestream = res.unwrap();

    let res = if is_range {
        let content_ranges = format!("bytes {}-{}/{}", offset, offset+length-1, filesz);
        client.write_get_object_response()
                    .request_route(output_route)
                    .request_token(output_token)
                    .status_code(206)
                    .content_range(content_ranges)
                    .accept_ranges("bytes")
                    .content_length(length as i64)
                    .body(bytestream)
                    .send()
                    .await
    } else {
        client.write_get_object_response()
                    .request_route(output_route)
                    .request_token(output_token)
                    .status_code(200)
                    .content_length(filesz as i64)
                    .body(bytestream)
                    .send()
                    .await
    };

    if res.is_err() {
        warn!("failed to send object content back to client, result: {:?}", res);
        return Ok(json!({"status_code": 200}))
    }
    debug!("GetObject success");
    Ok(json!({"status_code": 200}))
}

async fn head_object_handler(event: LambdaEvent<S3ObjectLambdaEvent>, env: Env) -> Result<Value, Error> {

    let context = event.payload.head_object_context.unwrap();
    let input_s3_url = context.input_s3_url.clone();
    let url = Url::parse(&input_s3_url).unwrap();
    let key = url.path().trim_start_matches('/');

    let res = env.vmap.query(key);
    if res.is_none() {
        info!("not found in repo map: {:?}", res);
        let resp = HeadObjectResponse {
            status_code: 404,
            error_code: Some("NotFound".to_string()),
            error_message: Some("ObjectNotFound".to_string()),
            headers: None,
        };
        return Ok(json!(resp));
    }
    let (matched_virtual_prefix, repo_bucket, repo_prefix, repo_object) = res.unwrap();
    let repo_key = format!("{}/{}", repo_prefix, repo_object);
    let cachefiledir = format!("{}/{}/{}", env.cache_dir, repo_bucket, repo_prefix);
    let cachefile = format!("{}/{}", cachefiledir, repo_object);
    info!("repo prefix: {}, repo_key: {}, cachefiledir: {}, cachefile: {}",
            repo_prefix, repo_key, cachefiledir, cachefile);
    tokio::fs::create_dir_all(cachefiledir).await.unwrap();
    let remote = repo::Remote::new(&env.region, &repo_bucket, &repo_key).await;
    let local = repo::Local::new(&cachefile, env.chunk_size, env.hdmode, false, false, Some(remote.clone()), false).await;
    let repo = local.clone();
    repo::CONTEXT.with(|c| *c.borrow_mut() = Some(local));

    let (repo_top, key) = get_repo_search_top_and_key(key, &matched_virtual_prefix);
    info!("repo_top {:?}, key {:?}", repo_top, key);
    let res = repo.file_stat(&key);
    let output;
    if res.is_none() {
        output = HeadObjectResponse {
            status_code: 404,
            error_code: Some("NotFound".to_string()),
            error_message: Some("ObjectNotFound".to_string()),
            headers: None,
        };
    } else {
        let stat = res.unwrap();
        if (stat.st_mode & libc::S_IFDIR) > 0 {
            output = HeadObjectResponse {
                status_code: 404,
                error_code: Some("NotFound".to_string()),
                error_message: Some("ObjectNotFound".to_string()),
                headers: None,
            };
        } else {
            let filesz = stat.st_size;
            let headers = HeadObjectHeaders {
                content_length: filesz as i32,
            };
            output = HeadObjectResponse {
                status_code: 200,
                error_code: None,
                error_message: None,
                headers: Some(headers),
            };
        }
    }
    Ok(json!(output))
}

async fn list_objects_v2_handler(event: LambdaEvent<S3ObjectLambdaEvent>, env: Env) -> Result<Value, Error> {

    let context = event.payload.list_objects_v2_context.unwrap();
    let input_s3_url = context.input_s3_url;
    let url = Url::parse(&input_s3_url).unwrap();
    let query: HashMap<_, _> = url.query_pairs().into_owned().collect();
    let search_prefix = query.get("prefix").unwrap();
    let continue_token = query.get("continuation-token")
                            .and_then(|x| base64::decode(x.as_bytes()).ok())
                            .and_then(|s| String::from_utf8(s).ok());
    let max_keys = query.get("max-keys")
                            .and_then(|x| x.parse::<usize>().ok())
                            .unwrap_or(1000);

    let res = env.vmap.query(search_prefix);
    if res.is_none() {
        info!("not found in repo map: {:?}", res);
        let output = ListObjectsResponse {
            status_code: 404,
            error_code: Some("NotFound".to_string()),
            error_message: Some("Not Found".to_string()),
            list_result_xml: "".to_string(),
        };
        return Ok(json!(output));
    }
    let (matched_virtual_prefix, repo_bucket, repo_prefix, repo_object) = res.unwrap();
    let repo_key = format!("{}/{}", repo_prefix, repo_object);
    let cachefiledir = format!("{}/{}/{}", env.cache_dir, repo_bucket, repo_prefix);
    let cachefile = format!("{}/{}", cachefiledir, repo_object);
    tokio::fs::create_dir_all(&cachefiledir).await.unwrap();
    info!("repo prefix: {}, repo_key: {}, cachefiledir: {}, cachefile: {}",
            repo_prefix, repo_key, cachefiledir, cachefile);
    let remote = repo::Remote::new(&env.region, &repo_bucket, &repo_key).await;
    let local = repo::Local::new(&cachefile, env.chunk_size, env.hdmode, false, false, Some(remote.clone()), false).await;
    let repo = local.clone();
    repo::CONTEXT.with(|c| *c.borrow_mut() = Some(local));

    let (repo_search_top, repo_search_key) = get_repo_search_top_and_key(search_prefix, &matched_virtual_prefix);
    info!("matched_virtual_prefix: {}, repo_prefix: {}, repo_search_top: {}, repo_search_key: {}",
        matched_virtual_prefix, repo_prefix, repo_search_top, repo_search_key);
    let top;
    if repo_search_top == "" {
        top = None;
    } else {
        top = Some(repo_search_top);
    }

    let mut v = repo.file_list(top);
    let last_end = filter_result(&mut v, repo_search_key, max_keys, continue_token);

    let mut contents = Vec::new();
    for f in &v {
        contents.push(Object {
            key: format!("{}{}", matched_virtual_prefix, f.0),
            last_modified: Some(DateTime::from_secs(f.1.st_mtime).fmt(Format::DateTime).unwrap()),
            etag: None,
            size: f.1.st_size as i32,
            checksum_algorighm: None,
            owner: None,
            storage_class: None,
        });
    }

    let mut has_more = false;
    let mut ct = None;
    if last_end.is_some() {
        has_more = true;
        ct = Some(base64::encode(last_end.unwrap().as_bytes()));
    }

    let result = ListBucketResult {
        name: repo_bucket,
        key_count: v.len() as i32,
        max_keys: max_keys as i32,
        is_truncated: has_more,
        contents: Some(contents),
        common_prefixes: None,
        continuation_token: ct,
        delimiter: None,
        encoding_type: None,
        next_continuation_token: None,
        prefix: None,
        start_after: None,
    };

    let xml = ListResultXml {
        list_result: result,
    };

    let output = ListObjectsResponse {
        status_code: 200,
        error_code: None,
        error_message: None,
        list_result_xml: xml_serde::to_string_custom(&xml, xml_serde::Options {include_schema_location: false}).unwrap().replace('\n', ""),
    };
    Ok(json!(output))
}

async fn function_handler(event: LambdaEvent<S3ObjectLambdaEvent>) -> Result<Value, Error> {

    // collect required env
    let region = std::env::var("AWS_REGION").unwrap();
    let cache_dir = std::env::var("S3ARCHIVEFS_CACHE_DIR").unwrap_or("/tmp".to_string());
    let chunk_size = std::env::var("S3ARCHIVEFS_CACHE_CHUNK_SIZE")
                                .unwrap_or_default()
                                .parse::<usize>()
                                .ok();
    let hdmode;
    if cache_dir.starts_with("/mnt") {
        info!("use all zero mode");
        hdmode = repo::HoleDetectMode::ALLZERO;
    } else {
        info!("use lseek mode");
        hdmode = repo::HoleDetectMode::LSEEK;
    }

    let vmap = PrefixVMap::new();

    let env = Env {
        region: region,
        vmap: vmap,
        cache_dir: cache_dir,
        chunk_size: chunk_size,
        hdmode: hdmode,
    };

    if event.payload.get_object_context.is_some() {

        info!("invoke GetObject");
        return get_object_handler(event, env).await;

    } else if event.payload.head_object_context.is_some() {

        info!("invoke HeadObject");
        return head_object_handler(event, env).await;

    } else if event.payload.list_objects_context.is_some() {

        info!("invoke ListObjects -- not support");
        let output = ListObjectsResponse {
            status_code: 400,
            error_code: Some("NotSupport".to_string()),
            error_message: Some("ListObjects not support on this endpoint, please use ListObjectsV2".to_string()),
            list_result_xml: "".to_string(),
        };
        return Ok(json!(output));
    } else if event.payload.list_objects_v2_context.is_some() {

        info!("invoke ListObjectV2");
        return list_objects_v2_handler(event, env).await;

    } else {
        panic!("no valid context in event");
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    env_logger::builder()
        .format_timestamp(None)
        .init();
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_target(false)
        .without_time()
        .init();

    run(service_fn(function_handler)).await
}
