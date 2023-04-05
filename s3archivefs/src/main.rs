use structopt::StructOpt;
use log::{info, error};
use aws_config::meta::region::RegionProviderChain;
use s3archivefs::repo::{Remote, Local, HoleDetectMode};
use s3archivefs::repo::CONTEXT;

#[derive(Debug, StructOpt)]
enum Cmd {
    Meta {
        #[structopt(help = "archivefs file")]
        file: String,
    },
    Install {
        #[structopt(short, display_order = 1, help = "region")]
        region: Option<String>,
        #[structopt(short, display_order = 2, help = "bucket")]
        bucket: String,
        #[structopt(short, display_order = 3, help = "key")]
        key: String,
        #[structopt(short, display_order = 4, help = "local archivefs file to install")]
        file: String,
    },
    Extract {
        #[structopt(short, display_order = 1, help = "region")]
        region: Option<String>,
        #[structopt(short, display_order = 2, help = "bucket")]
        bucket: String,
        #[structopt(short, display_order = 3, help = "key")]
        key: String,
        #[structopt(short, display_order = 4, help = "local archivefs cache")]
        cachefile: String,
        #[structopt(short="s", display_order = 5, help = "chunk size of local cache")]
        chunk_size: Option<usize>,
        #[structopt(short="t", display_order = 6, help = "file to extract")]
        filepath: String,
        #[structopt(short, display_order = 7, default_value = "/tmp", help = "local directory to save extract file")]
        localdir: String,
        #[structopt(short, display_order = 8, help = "hole detect with test all zeros")]
        zero: bool,
        #[structopt(short, display_order = 9, help = "force to use remote archive file")]
        force: bool,
        #[structopt(short="i", display_order = 10, help = "init root hierarchy")]
        init_root: bool,
    },
    List {
        #[structopt(short, display_order = 1, help = "region")]
        region: Option<String>,
        #[structopt(short, display_order = 2, help = "bucket")]
        bucket: String,
        #[structopt(short, display_order = 3, help = "key")]
        key: String,
        #[structopt(short, display_order = 4, help = "hole detect with test all zeros")]
        zero: bool,
        #[structopt(short, display_order = 5, help = "force to use remote archive file")]
        force: bool,
        #[structopt(short, display_order = 6, help = "local archivefs cache")]
        cachefile: String,
        #[structopt(short="s", display_order = 7, help = "chunk size of local cache")]
        chunk_size: Option<usize>,
        #[structopt(display_order = 8, help = "path of start point")]
        path: Option<String>,
    },
    Stat {
        #[structopt(short, display_order = 1, help = "region")]
        region: Option<String>,
        #[structopt(short, display_order = 2, help = "bucket")]
        bucket: String,
        #[structopt(short, display_order = 3, help = "key")]
        key: String,
        #[structopt(short, display_order = 4, help = "hole detect with test all zeros")]
        zero: bool,
        #[structopt(short, display_order = 5, help = "force to use remote archive file")]
        force: bool,
        #[structopt(short, display_order = 6, help = "local archivefs cache")]
        cachefile: String,
        #[structopt(short="s", display_order = 7, help = "chunk size of local cache")]
        chunk_size: Option<usize>,
        #[structopt(short="t", display_order = 8, help = "file to stat")]
        filepath: String,
    },
}

#[tokio::main]
async fn main() {

    env_logger::init_from_env(
        env_logger::Env::default()
            .filter_or(env_logger::DEFAULT_FILTER_ENV, format!("{}=info", env!("CARGO_PKG_NAME")))
    );

    let default_region = RegionProviderChain::default_provider()
                                    .region()
                                    .await;
    let opt = Cmd::from_args();
    match opt {
        Cmd::Meta {file} => {
            let local = Local::new(&file, None, HoleDetectMode::ALLZERO, false, false, None, false).await;
            local.print_superblock()
        }
        Cmd::Install {region, bucket, key, file} => {
            let remote = Remote::new(region
                            .or(default_region
                                .map(|r| r.as_ref().to_string())
                            )
                            .expect("no region config found in cli or profile")
                            .as_str(), &bucket, &key).await;

            let res = remote.intall_archivefs(&file).await;
            match res {
                Err(e) => {
                    error!("failed to create remote archivefs, {}", e);
                    return;
                },
                Ok(_) => {},
            }
        },
        Cmd::Extract {region, bucket, key, cachefile, chunk_size, filepath, localdir, zero, force, init_root} => {
            let remote = Remote::new(region
                            .or(default_region
                                .map(|r| r.as_ref().to_string())
                            )
                            .expect("no region config found in cli or profile")
                            .as_str(), &bucket, &key).await;
            let hdmode;
            if zero {
                hdmode = HoleDetectMode::ALLZERO;
            } else {
                hdmode = HoleDetectMode::LSEEK;
            }

            let local = Local::new(&cachefile, chunk_size, hdmode, force, init_root, Some(remote.clone()), false).await;
            let _l = local.clone();
            CONTEXT.with(|c| *c.borrow_mut() = Some(local));

            let filename = filepath.split("/").last();
            if filename.is_none() {
                error!("invalid file path {}", &filepath);
            }
            let output_path = localdir + "/" + filename.unwrap();
            info!("extract {} from archive to {}", &filepath, &output_path);
            let res = _l.extract_one(&filepath, &output_path);
            match res {
                Err(e) => {
                    error!("failed to extract file {}, error: {}", &filepath, e);
                    return;
                },
                Ok(_) => {},
            }
        },
        Cmd::List {region, bucket, key, zero, force, cachefile, chunk_size, path} => {
            let remote = Remote::new(region
                            .or(default_region
                                .map(|r| r.as_ref().to_string())
                            )
                            .expect("no region config found in cli or profile")
                            .as_str(), &bucket, &key).await;
            let hdmode;
            if zero {
                hdmode = HoleDetectMode::ALLZERO;
            } else {
                hdmode = HoleDetectMode::LSEEK;
            }

            let local = Local::new(&cachefile, chunk_size, hdmode, force, true, Some(remote.clone()), false).await;
            let _l = local.clone();
            CONTEXT.with(|c| *c.borrow_mut() = Some(local));
            _l.print_list(path);
        },
        Cmd::Stat {region, bucket, key, zero, force, cachefile, chunk_size, filepath} => {
            let remote = Remote::new(region
                            .or(default_region
                                .map(|r| r.as_ref().to_string())
                            )
                            .expect("no region config found in cli or profile")
                            .as_str(), &bucket, &key).await;
            let hdmode;
            if zero {
                hdmode = HoleDetectMode::ALLZERO;
            } else {
                hdmode = HoleDetectMode::LSEEK;
            }

            let local = Local::new(&cachefile, chunk_size, hdmode, force, true, Some(remote.clone()), false).await;
            let _l = local.clone();
            CONTEXT.with(|c| *c.borrow_mut() = Some(local));
            _l.print_stat(&filepath);
        },
    }
}
