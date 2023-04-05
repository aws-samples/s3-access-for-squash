# Access S3 archive in squash manner
Tools to allow user access files squashed in S3 archive object on demand.

Typically in massive small text files archive use case, likely you have a large local source code tree wants to be archived to S3.

It will be inefficent and costly if user inject/ingest tens of thouands files direct to/from S3.

With S3 Squash Archive Tools, user can compress and aggregate small files into a single big archive object before upload to S3.

When a individual file needs to be access later some time, user can fetch through S3 GetObject API which in behind a [S3 Object Lambda](https://docs.aws.amazon.com/AmazonS3/latest/userguide/transforming-objects.html) function will automatically locate file contents inside of archive object in S3 and extract the only necessary bytes back to user.

This in line with the normal way user interact with S3 and will not break current toolchains.

## Features
- Squashfs as self-described archive container, no extra metadata.
- Aggregate small files into single big one for long term store in S3.
- Perserve all posix file attributes and extended attributes.
- Extract content with S3 native API specification.
- Implemented GetObject, HeadObject and ListObjectsV2 through [S3 Object Lambda](https://docs.aws.amazon.com/AmazonS3/latest/userguide/transforming-objects.html).
- Cache mechanisum to minimize API calls to S3.

## Modules

[```s3archivefs```](s3archivefs) Rust wrapper of [squash-tools-ng](https://github.com/AgentD/squashfs-tools-ng)

[```s3archivefs-lambda```](s3archivefs-lambda) Reference implementation of S3 Object Lambda to fetch data in archive object

[```s3archivefs-fuse```](s3archivefs-fuse) Local filesystem access for archive object by using FUSE

## Reference
https://docs.aws.amazon.com/AmazonS3/latest/userguide/transforming-objects.html

## Notice
:warning: S3 Squash Archive target for massive small files archive use case, typically once injected seldom access.
if your use case need high frequency access full dataset, please upload your file to Amazon S3 as normal way. standard Amazon S3 access provide file to S3 object 1x1 mapping, you can get MAX throughput from S3 and it's always most cost effective way.

## Security
See [CONTRIBUTING](CONTRIBUTING.md#security-issue-notifications) for more information.

## License
This project is licensed under the Apache-2.0 License.
