use serde::{Deserialize, Serialize};

// follow model defined by:
// https://docs.aws.amazon.com/AmazonS3/latest/userguide/olap-writing-lambda.html#olap-getobject-response
// ref: https://docs.rs/aws-sdk-s3/0.21.0/src/aws_sdk_s3/output.rs.html
#[derive(Debug, Serialize, Deserialize)]
pub struct ListBucketResult {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Prefix", skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    #[serde(rename = "StartAfter", skip_serializing_if = "Option::is_none")]
    pub start_after: Option<String>,
    #[serde(rename = "ContinuationToken", skip_serializing_if = "Option::is_none")]
    pub continuation_token: Option<String>,
    #[serde(rename = "NextContinuationToken", skip_serializing_if = "Option::is_none")]
    pub next_continuation_token: Option<String>,
    #[serde(rename = "KeyCount")]
    pub key_count: i32,
    #[serde(rename = "MaxKeys")]
    pub max_keys: i32,
    #[serde(rename = "Delimiter", skip_serializing_if = "Option::is_none")]
    pub delimiter: Option<String>,
    #[serde(rename = "EncodingType", skip_serializing_if = "Option::is_none")]
    pub encoding_type: Option<String>,
    #[serde(rename = "IsTruncated")]
    pub is_truncated: bool,
    #[serde(rename = "Contents", skip_serializing_if = "Option::is_none")]
    pub contents: Option<Vec<Object>>,
    #[serde(rename = "CommonPrefixes", skip_serializing_if = "Option::is_none")]
    pub common_prefixes: Option<Vec<CommonPrefixes>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Object {
    #[serde(rename = "Key")]
    pub key: String,
    #[serde(rename = "LastModified", skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
    #[serde(rename = "ETag", skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    #[serde(rename = "ChecksumAlgorithm", skip_serializing_if = "Option::is_none")]
    pub checksum_algorighm: Option<String>,
    #[serde(rename = "Size")]
    pub size: i32,
    #[serde(rename = "Owner", skip_serializing_if = "Option::is_none")]
    pub owner: Option<Owner>,
    #[serde(rename = "StorageClass", skip_serializing_if = "Option::is_none")]
    pub storage_class: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Owner {
    #[serde(rename = "Prefix")]
    pub display_name: String,
    #[serde(rename = "ID")]
    pub id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CommonPrefixes {
    #[serde(rename = "Prefix")]
    pub prefix: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ListResultXml {
    #[serde(rename = "ListBucketResult")]
    pub list_result: ListBucketResult,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ListObjectsResponse {
    #[serde(rename = "statusCode")]
    pub status_code: i32,
    #[serde(rename = "errorCode", skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(rename = "errorMessage", skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(rename = "listResultXml")]
    pub list_result_xml: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HeadObjectHeaders {
    #[serde(rename = "Content-Length")]
    pub content_length: i32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HeadObjectResponse {
    #[serde(rename = "statusCode")]
    pub status_code: i32,
    #[serde(rename = "errorCode", skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(rename = "errorMessage", skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    pub headers: Option<HeadObjectHeaders>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;
    use aws_smithy_types::DateTime;
    use aws_smithy_types::date_time::Format;

    //#[test]
    fn test_serde() {
        let obj = Object {
            key: "index/repo/aha.sqsh".to_string(),
            last_modified: Some(DateTime::from(SystemTime::now()).fmt(Format::DateTime).unwrap()),
            etag: Some("7f07d92fe5d6ab7e6373023ce405cb50-12".to_string()),
            size: 185786368,
            checksum_algorighm: None,
            owner: None,
            storage_class: None,
        };
        let xml = xml_serde::to_string(&obj).unwrap();
        println!("{}", xml);
    }

    #[test]
    fn test_ser_de() {
        let mut contents = Vec::new();
        let obj = Object {
            key: "output.sqsh".to_string(),
            last_modified: Some(DateTime::from(SystemTime::now()).fmt(Format::DateTime).unwrap()),
            etag: Some("e0f28a5fb7b5a9462dad1811b91cf495-23".to_string()),
            size: 185786368,
            checksum_algorighm: None,
            owner: None,
            storage_class: None,
        };
        contents.push(obj);
        let obj = Object {
            key: "index/repo/aha.sqsh".to_string(),
            last_modified: Some(DateTime::from(SystemTime::now()).fmt(Format::DateTime).unwrap()),
            etag: Some("7f07d92fe5d6ab7e6373023ce405cb50-12".to_string()),
            size: 185786368,
            checksum_algorighm: None,
            owner: None,
            storage_class: None,
        };
        contents.push(obj);
        let result = ListBucketResult {
            name: "aha30".to_string(),
            key_count: 2,
            max_keys: 1000,
            is_truncated: false,
            contents: Some(contents),
            common_prefixes: None,
            continuation_token: None,
            delimiter: None,
            encoding_type: None,
            next_continuation_token: None,
            prefix: None,
            start_after: None,
        };

        println!("{:?}", result);
        let xml = xml_serde::to_string_custom(&result, xml_serde::Options {include_schema_location: false}).unwrap();
        println!("{}", xml);
        /*
        let output = ListObjectsResponse {
            status_code: 200,
            error_code: None,
            error_message: None,
            list_result_xml: serde_xml_rs::ser::to_string(&result).unwrap(),
        };
        ret = json!(output);
        */
    }
}
