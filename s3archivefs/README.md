# S3 Archive FS

Rust wrapper of [squash-tools-ng](https://github.com/AgentD/squashfs-tools-ng) to implement underlayer container for archive data.

## How to build

### Build squash-tools-ng

Install essential packages
```
sudo yum install autoconf libtool automake clang lzo-devel xz-devel lz4-devel libzstd-devel bzip2-devel
```
Get latest source code
```
git clone https://github.com/AgentD/squashfs-tools-ng
```
Configure
```
cd squashfs-tools-ng && ./autogen.sh && ./configure --enable-static=no
```
Build & install
```
make && sudo make install
```

For more information, see:

https://github.com/AgentD/squashfs-tools-ng#getting-and-building-the-source-code

### Install Rust (Optional)
```
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```
### Build binary

inside of project folder, run:
```
cargo build --release
```
you will find binary at `target/release`

## How to run
### Load libsquashfs.so correctly
By default, ```squash-tools-ng``` will be installed at ```/usr/local/bin``` and ```libsquashfs.so``` will be installed at ```/usr/local/lib```.

if you get error while execute squash-tools-ng tools or s3archivefs tools:
```
error while loading shared libraries: libsquashfs.so.1: cannot open shared object file: No such file or directory
```

add ```/usr/local/lib``` to your ```LD_LIBRARY_PATH```

```
export LD_LIBRARY_PATH=$LD_LIBRARY_PATH:/usr/local/lib
export PATH=$PATH:/usr/local/bin
````
or you can do 
```
sudo ldconfig /usr/local/lib
```

### Package your local folder with gensquashfs
Before we upload archive to S3, we need firstly package your local folder into squashfs format by ```gensquashfs``` or any other tools can generate a squashfs image.

For example, you have a local copy of latest linux kernel 6.1 source tree need to be archived, you simply run:
```
gensquashfs --pack-dir ./linux-6.1 --block-size 1048576 --keep-time linux-6.1.sqfs
```

Default block size of ```gensquashfs``` is 128 KiB, which is somewhat too small, we recommand set this to 1 MiB which is the max block size squashfs supported.

When laterly data need to be retrived from archive in S3, larger block size could be more efficent.

Be sure you understand what you are doing and it's affect, before tune any ```gensquashfs``` parameters including block size.

### Install (Upload) your archive to S3
Upload your generated squashfs archive to S3 by ```s3archivefs install``` CLI:
```
s3archivefs install -b <your bucket> -k <prefix/object.name> -f <your local archive file>
```
**DO NOT** upload archive direct with AWS console or AWS CLI, ```s3archivefs install``` needs to extract superblock from squashfs image and save it as [User-defined object metadata](https://docs.aws.amazon.com/AmazonS3/latest/userguide/UsingMetadata.html#UserMetadata) together with archive object.

Fine tune s3archivefs install procedure with:

| Environment | Description | Default |
| ----------- | ----------- | ------- |
| S3ARCHIVEFS_STORAGE_CLASS | S3 storage class to install to, possible values:<br/>STANDARD \| INTELLIGENT_TIERING ( INT ) \| GIR | STANDARD |
| S3ARCHIVEFS_MPU_CHUNK_SIZE | multipart upload part size in byte | 5242880 |

### Verify your installation
You can verify your archive repo installation by:
```
s3archivefs list -b <your bucket> -k <prefix/object.name> -c <local cache file>
```
```s3archivefs list``` only download metadata section of archive object, which in consequence create a local cache file in sparse format, this can be verify as following:
```
$ du --apparent-size -h cache.sqfs
149M	cache.sqfs
$
$ du -h cache.sqfs
1.9M	cache.sqfs
```
For more understanding of file format, check [Squashfs Binary Format](https://dr-emann.github.io/squashfs/squashfs.html)

### Extract file attributes in archive
```
s3archivefs stat -b  <your bucket> -k <prefix/object.name> -c <local cache file> -t /Documentation/filesystems/squashfs.rst
```

### Extract content from archive
```
s3archivefs extract -b  <your bucket> -k <prefix/object.name> -c <local cache file> -t /Documentation/filesystems/squashfs.rst -l /tmp
```
Find extracted local file copy in ```/tmp``` with all attributes preserved.

NOTE:

You can always point to same local cache file with ```-c```, s3archvefs will check local cache before retrieve necessary bytes from remote archive in S3, to minimize network usage.

## Security
See [CONTRIBUTING](../CONTRIBUTING.md#security-issue-notifications) for more information.

## License
This project is licensed under the Apache-2.0 License.
