use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
};
use ultrafinance_core::logos::ImageAsset;
pub struct Storage {
    directory: Option<PathBuf>,
    bucket: Option<String>,
    base: reqwest::Url,
}
impl Storage {
    pub fn new(directory: Option<PathBuf>, bucket: Option<String>, base: &str) -> Result<Self> {
        if directory.is_some() == bucket.is_some() {
            bail!("choose exactly one of --directory or --bucket");
        }
        if let Some(bucket) = &bucket
            && (bucket.is_empty()
                || !bucket.bytes().all(|c| {
                    c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-' || c == b'.'
                }))
        {
            bail!("invalid S3 bucket name");
        }
        let base = reqwest::Url::parse(base)?;
        if base.scheme() != "https"
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
            || base.path() != "/"
        {
            bail!("public base URL must be a credential-free HTTPS origin");
        }
        Ok(Self {
            directory,
            bucket,
            base,
        })
    }
    pub fn publish(
        &self,
        bytes: &[u8],
        mime: &str,
        ext: &str,
        width: u32,
        height: u32,
        original: bool,
    ) -> Result<ImageAsset> {
        let id: String = Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        // One immutable location per content hash, shared across associations.
        let key = format!("logos/assets/{id}.{ext}");
        if let Some(directory) = &self.directory {
            let path = directory.join(&key);
            std::fs::create_dir_all(path.parent().unwrap())?;
            let mut temporary = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
            temporary.write_all(bytes)?;
            temporary.persist(&path).map_err(|e| e.error)?;
        } else if let Some(bucket) = &self.bucket {
            let mut temporary = tempfile::NamedTempFile::new()?;
            temporary.write_all(bytes)?;
            // No shell; preserve the operator's standard AWS profile/region and
            // instance/task credentials. Never print subprocess credential output.
            let output = Command::new("aws")
                .args(["s3", "cp"])
                .arg(temporary.path())
                .arg(format!("s3://{bucket}/{key}"))
                .args([
                    "--content-type",
                    mime,
                    "--cache-control",
                    "public,max-age=31536000,immutable",
                    "--content-disposition",
                    if original && mime == "image/svg+xml" {
                        "attachment"
                    } else {
                        "inline"
                    },
                    "--only-show-errors",
                ])
                .stdin(Stdio::null())
                .output()
                .context("cannot run AWS CLI to upload logo")?;
            if !output.status.success() {
                bail!("S3 logo upload failed ({})", output.status);
            }
        }
        Ok(ImageAsset {
            id,
            url: self.base.join(&key)?.to_string(),
            mime_type: mime.into(),
            width,
            height,
            byte_length: bytes.len() as u64,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stores_immutable_files_with_matching_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let storage = Storage::new(
            Some(directory.path().into()),
            None,
            "https://assets.example.com",
        )
        .unwrap();
        let asset = storage
            .publish(b"example bytes", "image/webp", "webp", 16, 16, false)
            .unwrap();
        let again = storage
            .publish(b"example bytes", "image/webp", "webp", 16, 16, true)
            .unwrap();
        assert_eq!(asset.id, again.id);
        assert_eq!(asset.url, again.url);
        let path = Url::parse(&asset.url)
            .unwrap()
            .path()
            .trim_start_matches('/')
            .to_owned();
        assert_eq!(
            std::fs::read(directory.path().join(path)).unwrap(),
            b"example bytes"
        );
        assert_eq!(asset.byte_length, 13);
        assert!(
            Storage::new(
                Some(directory.path().into()),
                None,
                "https://user:secret@assets.example.com"
            )
            .is_err()
        );
        assert!(Storage::new(None, None, "https://assets.example.com").is_err());
    }
    use reqwest::Url;
}
