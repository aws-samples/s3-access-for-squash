# S3 Object Lambda to access data in archived squashfs object with S3 API

Reference implementation of S3 Object Lambda to fetch data in squashfs archive object upload by ```s3archivefs```

Supported S3 API through S3 Object Lambda Access Point:
- ```GetObject```
- ```HeadObject```
- ```ListObjectsV2```
- ```ListObject``` is **NOT** supported, please use ```ListObjectsV2```
- ```PutObject``` is **NOT** supported through S3 Object Lambda, any PutObject API call will be passthrough to [Supporting Access Point](https://docs.aws.amazon.com/AmazonS3/latest/userguide/olap-create.html)

For more details on the concept of S3 Object Lambda and how S3 Object Lambda works, see [Transforming objects with S3 Object Lambda](https://docs.aws.amazon.com/AmazonS3/latest/userguide/transforming-objects.html).

## Deploy essential shared library

```s3archivefs``` relying on ```libsquashfs.so```, before you deploy lambda function, package and deploy essential shared library as a lambda layer:
```
$ mkdir -p lambda-layer/lib && cd lambda-layer/lib
$ cp /usr/local/lib/libsquashfs.so ./
$ ln -s libsquashfs.so libsquashfs.so.1
```
Depending on the compressor you enabled and desired to use, you may also need to copy corresponding library
```
$ cp /usr/lib64/liblz4.so ./
$ ln -s liblz4.so liblz4.so.1

$ cp /usr/lib64/liblzma.so ./
$ ln -s liblzma.so liblzma.so.5

$ cp /usr/lib64/libz.so ./
$ ln -s libz.so libz.so.1

$ cp /usr/lib64/libzstd.so ./
$ ln -s libzstd.so libzstd.so.1
```
After we collected all library, package ```lib``` directory with ```zip```
```
$ cd ..
$ zip layer.zip ./lib/*
  adding: lib/liblz4.so (deflated 69%)
  adding: lib/liblz4.so.1 (deflated 69%)
  adding: lib/liblzma.so (deflated 57%)
  adding: lib/liblzma.so.5 (deflated 57%)
  adding: lib/libsquashfs.so (deflated 68%)
  adding: lib/libsquashfs.so.1 (deflated 68%)
  adding: lib/libz.so (deflated 63%)
  adding: lib/libz.so.1 (deflated 63%)
  adding: lib/libzstd.so (deflated 54%)
  adding: lib/libzstd.so.1 (deflated 54%)
```
To upload and deploy ```layer.zip```, please follow steps in [Creating layer content](https://docs.aws.amazon.com/lambda/latest/dg/configuration-layers.html#configuration-layers-upload).
When lambda deployed and run, shared library can be loacate at ```/opt/lib```.

## Build and Deploy Lambda

You need to build and package rust binary locally before deploy as a lambda

### Method 1. Build and package lambda

Following the steps in [Package and upload the app](https://docs.aws.amazon.com/sdk-for-rust/latest/dg/lambda.html#lambda-step3).

### Method 2. Use cargo-lambda

cargo-lambda helps you easily build and deploy lambda with Rust code.

Basically you need:
```
pip3 install cargo-lambda
# run build in dir s3archivefs-lambda
cargo lambda build --release
# run deploy at top level project home
cargo lambda deploy
```
Check [Installation](https://www.cargo-lambda.info/guide/installation.html) and [Getting Started](https://www.cargo-lambda.info/guide/getting-started.html) for more details.

## Create S3 Object Lambda Access Point
This lambda **CAN NOT** running standalone, following steps to [Creating Object Lambda access points](https://docs.aws.amazon.com/AmazonS3/latest/userguide/olap-create.html).

## Lambda environment variables
| Environment | Description | Default |
| ----------- | ----------- | ------- |
| S3ARCHIVEFS_CACHE_DIR | cache file location, when working with EFS, it could be set to /mnt/\<EFS mountpoint\> | /tmp |
| S3ARCHIVEFS_CACHE_CHUNK_SIZE | cache chunk size, align to log2 floor<br/>if not set or too small, use block size from super block | N/A |
| S3ARCHIVEFS_PREFIX_VMAP{1..20} | preload virtual prefix map, if your mapping count exceed 20, set ```PREFIX_VMAP_EXT_FILE```<br/>syntax: ```virtual/prefix\|s3://bucket/prefix/object``` | N/A |
| S3ARCHIVEFS_PREFIX_VMAP_EXT_FILE | file path of virutal prefix map, each line per mapping<br/>syntax: ```virtual/prefix\|s3://bucket/prefix/object``` | N/A |

## Cache file location consideration
To maximum read performance and minimize network traffic, s3archive designed with cache mechanism,

in a lambda deployment, you can either put local cache file on

```/tmp``` the local temparory storage space which can only be accessed by single lambda instance.

or

```/mnt/<EFS mountpoint>``` Elastic Filesystem which same copy of data could be shared concurrently by multiple lambda instances.

the location of local cache file controlled by ```S3ARCHIVEFS_CACHE_DIR```.

## Configure your S3 Object Lambda to access EFS
To work with EFS, you need to connect your lambda to private subnet in a VPC:

1. [Configuring a Lambda function to access resources in a VPC](https://docs.aws.amazon.com/lambda/latest/dg/configuration-vpc.html)
2. [Configuring file system access for Lambda functions](https://docs.aws.amazon.com/lambda/latest/dg/configuration-filesystem.html)

## Give internet access
You will need a NAT Gateway to let GetObject operation send data back, please follow steps below:

[Give internet access to a Lambda function that's connected to an Amazon VPC](https://aws.amazon.com/premiumsupport/knowledge-center/internet-access-lambda-function/)

## Security
See [CONTRIBUTING](../CONTRIBUTING.md#security-issue-notifications) for more information.

## License
This project is licensed under the Apache-2.0 License.
