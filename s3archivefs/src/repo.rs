use std::path::Path;
use std::rc::Rc;
use std::io::{Error, ErrorKind};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use std::cell::RefCell;
use std::pin::Pin;
use std::task::Poll;
use std::future::Future;
use tokio::io::AsyncReadExt;
use tokio::io::SeekFrom;
use tokio::fs::File;
use tokio::io::AsyncSeekExt;
use tokio::io::AsyncWriteExt;
use log::{debug, warn, error};
use aws_smithy_http::byte_stream::ByteStream;
use fs4::tokio::AsyncFileExt;
use crate::transfer::TransferManager;
use crate::bindings::sqfs_super_t;
use crate::squashfs_v1::Archive;

thread_local! {
    pub static CONTEXT: RefCell<Option<Local>> = RefCell::new(None);
}

#[derive(Debug, Clone)]
pub struct Remote {
    tm: TransferManager,
    bucket: String,
    key: String,
}

impl Remote { 

    pub async fn new(region: &str, bucket: &str, key: &str) -> Self {
        Self {
            tm: TransferManager::new(region).await,
            bucket: bucket.to_string(),
            key: key.to_string(),
        }
    }

    // get superblock from object metadata
    pub async fn get_metadata(&self) -> Result<(Vec<u8>, i64), Error> {
        let meta = self.tm.head_object(&self.bucket, &self.key).await?;
        let filesize = meta.content_length();
        let null = String::from("");
        let encoded: Option<&String> = meta.metadata().map(|m| {
                m.get("s3archivefs-superblock").unwrap_or(&null)
            });
        let sb_bin = encoded
                    .map(|s| base64::decode(s)
                            .map_err(|e| {
                                error!("failed to get superblock bin from metadata, error: {}", e);
                                Error::new(ErrorKind::NotFound, "failed to get superblock bin from metadata")
                            })
                    ).unwrap()?;
                    
        if sb_bin.len() != std::mem::size_of::<sqfs_super_t>() {
            error!("size of decoded super block vec {}, sqfs_super_t size {}", sb_bin.len(), std::mem::size_of::<sqfs_super_t>());
            return Err(Error::new(ErrorKind::InvalidData, "incorrect superblock size"));
        }

        Ok((sb_bin, filesize))

    }

    // get a range
    pub async fn get_range(&self, start: usize, end: usize) -> Result<ByteStream, Error> {
        let range = Some(format!("bytes={}-{}", start, end));
        debug!("range to get: {:?}", range.as_ref().unwrap());
        self.tm.download_object(&self.bucket, &self.key, range).await
    }

    pub async fn intall_archivefs(&self, from: &str) -> Result<(), Error> {
        let mut file = File::open(from).await?;
        let mut buf = vec![0; std::mem::size_of::<sqfs_super_t>()];
        file.read_exact(&mut buf).await?;
        let encoded = base64::encode(buf);
        let metadata = Some(HashMap::from([("S3ARCHIVEFS-SUPERBLOCK".to_string(), encoded)]));
        self.tm.upload_object(from, &self.bucket, &self.key, 0, metadata).await?;
        Ok(())
    }
}

#[derive(Copy, Clone, PartialEq)]
pub enum HoleDetectMode {
    ALLZERO,
    LSEEK,
}

#[derive(Clone)]
pub struct Local {
    remote: Option<Remote>,
    filepath: String,
    arcfs: Rc<Archive>,
    sb: sqfs_super_t,
    hdmode: HoleDetectMode,
    chunk_log: usize,
}

unsafe impl Send for Local {}
unsafe impl Sync for Local {}

const MAX_CHUNK_SIZE: usize = 0x1_0000_0000;

impl Local {

    pub async fn new(filepath: &str, opt_chunk_size: Option<usize>, hdmode: HoleDetectMode, force: bool, init_root: bool, remote: Option<Remote>) -> Self {

        let path = Path::new(filepath);

        let chunk_size = opt_chunk_size
                            .map(|x|
                                if x > MAX_CHUNK_SIZE {
                                    MAX_CHUNK_SIZE
                                } else {
                                    x
                                }
                            ).unwrap_or_default();

        let exists = path.try_exists().expect("failed to check existance");
        debug!("local cache {} exists {}", filepath, exists);
        if !exists || force {
            if remote.is_none() {
                panic!("both local and remote repo not exist, can not continue");
            }
            let (sb_bin, filesize) = remote.as_ref().unwrap().get_metadata().await.expect("unable to read superblock from remote");

            let mut file = tokio::fs::OpenOptions::new()
                            .read(true)
                            .write(true)
                            .create(true)
                            .truncate(force)
                            .open(path).await.expect("failed to create local repo sparse file");

            // create sparse file based on file size
            file.seek(SeekFrom::Start((filesize - 1) as u64)).await.expect("failed to seek file");
            file.write_all(&[0]).await.expect("failed to write sparse file");

            // write superblock
            file.seek(SeekFrom::Start(0)).await.expect("failed to seek file");
            file.write_all(&sb_bin).await.expect("failed to write superblock to local");

            let mut superblock = std::mem::MaybeUninit::<sqfs_super_t>::uninit();
            let sb = unsafe { 
                std::ptr::copy_nonoverlapping(sb_bin.as_ptr() as *const sqfs_super_t, superblock.as_mut_ptr(), 1);
                superblock.assume_init()
            };
            let mut meta_start = sb.inode_table_start;
            let block_log = sb.block_log as usize;
            let block_size = sb.block_size as usize;
            let chunk_log;
            if chunk_size <= block_size {
                chunk_log = block_log;
            } else {
                chunk_log = (chunk_size as f32).log2().floor() as usize;
            }

            // align to block size boundary
            meta_start = (meta_start >> chunk_log) << chunk_log;
            let stream = remote.as_ref()
                            .unwrap()
                            .get_range(meta_start as usize, (filesize - 1) as usize)
                            .await
                            .expect("unable to read superblock from remote");

            file.seek(SeekFrom::Start(meta_start)).await.expect("failed to seek file");
            let mut reader = tokio::io::BufReader::new(stream.into_async_read());
            let mut writer = tokio::io::BufWriter::new(&mut file);
            tokio::io::copy(&mut reader, &mut writer).await.expect("failed to finish io copy");
            writer.flush().await.expect("failed to flush data to local");
        }

        let arcfs = Rc::new(Archive::new_from_sparse(filepath, init_root));
        let sb = arcfs.get_sb();
        let block_log = sb.block_log;
        let block_size = sb.block_size as usize;
        let chunk_log;
        if chunk_size <= block_size {
            chunk_log = block_log as usize;
        } else {
            chunk_log = (chunk_size as f32).log2().floor() as usize;
        }
        debug!("block size: {}, block_log: {}, chunk_size: {}, chunk_log: {}",
            block_size, block_log, (1 as usize) << chunk_log, chunk_log);

        Self {
            remote: remote,
            filepath: filepath.to_string(),
            sb: sb,
            arcfs: arcfs,
            hdmode: hdmode,
            chunk_log: chunk_log,
        }
    }

    pub fn hdmode(&self) -> HoleDetectMode {
        self.hdmode
    }

    pub fn request_remote_data_task(&self, start_offset: usize, req_size: usize) -> Result<(), Error> {

        if self.remote.is_none() {
            return Ok(());
        }

        let aligned_start = (start_offset >> self.chunk_log) << self.chunk_log;

        let chunk_size = (1 as usize) << self.chunk_log;
        let aligned_end = (((start_offset + req_size) >> self.chunk_log) << self.chunk_log) + chunk_size;
        debug!("align end to block boundary offset {} - {}", aligned_start, aligned_end);

        let remote = self.remote.clone();
        let filepath = self.filepath.clone();
        std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(async {
                let stream = remote.as_ref().unwrap().get_range(aligned_start, aligned_end - 1).await?;

                let mut file = tokio::fs::OpenOptions::new()
                                .write(true)
                                .open(&filepath)
                                .await?;
                file.seek(SeekFrom::Start(aligned_start as u64)).await?;
                // 10s for file lock wait timeout
                let flock = FileLock::new(&file, Duration::new(10, 0));
                flock.await?;
                let mut reader = tokio::io::BufReader::new(stream.into_async_read());
                let mut writer = tokio::io::BufWriter::new(&mut file);
                tokio::io::copy(&mut reader, &mut writer).await?;
                writer.flush().await?;
                Ok::<(), Error>(())
            })
        }).join().unwrap()?;

        Ok(())
    }

    pub fn extract_one(&self, path: &str, outpath: &str) -> Result<usize, Error> {
        self.arcfs.extract_one(path, outpath)
    }

    pub fn print_list(&self, path: Option<String>) {
        self.arcfs.print_list(path);
    }

    pub fn print_stat(&self, filepath: &str) {
        self.arcfs.print_file_stat(filepath);
    }

    pub fn file_list(&self, path: Option<String>) -> Vec<(String, libc::stat64)> {
        unsafe {
            self.arcfs.file_list(path)
        }
    }

    pub fn file_stat(&self, filepath: &str) -> Option<libc::stat64> {
        unsafe {
            self.arcfs.file_stat(filepath)
        }
    }

    pub fn is_metadata_area(&self, offset: usize) -> bool {
        if offset < self.sb.inode_table_start as usize {
            return false;
        } else if offset > self.arcfs.get_archive_file_size() {
            panic!("requested offset {} is large than file size", offset);
        }
        return true;
    }

    pub fn print_superblock(&self) {
        let filesz = std::fs::metadata(&self.filepath).unwrap().len();
        println!("archive size:\t{}", filesz);
        println!("metadata size:\t{}", filesz - self.sb.inode_table_start);
        println!("======== super block ========");
        println!("inode count:\t{}", self.sb.inode_count);
        println!("block size:\t{}", self.sb.block_size);
        match self.sb.compression_id {
            1 => println!("compressor:\tZLIB"),
            2 => println!("compressor:\tLZMA"),
            3 => println!("compressor:\tLZO"),
            4 => println!("compressor:\tXZ"),
            5 => println!("compressor:\tLZ4"),
            6 => println!("compressor:\tZSTD"),
            _ => println!("unkown compressor"),
        }
        println!("bytes used:\t{}", self.sb.bytes_used);
        println!("inode table:\t{}", self.sb.inode_table_start);
        println!("dir table:\t{}", self.sb.directory_table_start);
        println!("fragment table:\t{}", if self.sb.fragment_table_start == u64::MAX {0} else {self.sb.fragment_table_start});
        println!("export table:\t{}", if self.sb.export_table_start == u64::MAX {0} else {self.sb.export_table_start});
        println!("id table:\t{}", self.sb.id_table_start);
        println!("xattr table:\t{}", if self.sb.xattr_id_table_start == u64::MAX {0} else {self.sb.xattr_id_table_start});
    }
}

struct FileLock<'a> {
    file: &'a tokio::fs::File,
    start: Instant,
    timeout: Duration,
}

impl<'a> FileLock<'a> {
    fn new(file: &'a tokio::fs::File, timeout: Duration) -> Self {
        Self {
            file: file,
            start: Instant::now(),
            timeout: timeout,
        }
    }
}

impl<'a> Future for FileLock<'a> {
    type Output = Result<(), Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut core::task::Context<'_>) -> Poll<Self::Output> {

        match self.file.try_lock_exclusive() {
            Ok(_) => {
                return Poll::Ready(Ok(()));
            },
            Err(e) => {
                if e.kind() != ErrorKind::WouldBlock {
                    warn!("unhandled error occur when try lock exlusive: {}", e);
                    return Poll::Ready(Err(Error::new(ErrorKind::Other, "try lock exlusive failed")));
                }
            },
        }

        if self.start.elapsed() >= self.timeout {
            return Poll::Ready(Err(Error::new(ErrorKind::TimedOut, "timeout")));
        }

        cx.waker().wake_by_ref();
        Poll::Pending
    }
}
