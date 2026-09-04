use anyhow::{Context, Result};
use aws_sdk_s3::Client;
use chrono::DateTime;

use crate::app::{
    BucketInfo, DownloadReport, ObjectDetail, ObjectInfo, StorageClass, UploadReport,
};
use crate::config::StaticCredentials;

fn map_storage_class(sc: &aws_sdk_s3::types::ObjectStorageClass) -> StorageClass {
    use aws_sdk_s3::types::ObjectStorageClass as Aws;
    match sc {
        Aws::Standard => StorageClass::Standard,
        Aws::ReducedRedundancy => StorageClass::ReducedRedundancy,
        Aws::IntelligentTiering => StorageClass::IntelligentTiering,
        Aws::Glacier => StorageClass::Glacier,
        Aws::GlacierIr => StorageClass::GlacierIr,
        Aws::StandardIa => StorageClass::StandardIa,
        Aws::OnezoneIa => StorageClass::OneZoneIa,
        Aws::ExpressOnezone => StorageClass::ExpressOnezone,
        Aws::Outposts => StorageClass::Outposts,
        Aws::DeepArchive => StorageClass::DeepArchive,
        Aws::Snow => StorageClass::Snow,
        other => StorageClass::Unknown(other.as_str().to_string()),
    }
}

pub struct S3Client {
    pub client: Client,
    pub region: String,
}

impl S3Client {
    pub fn from_client(client: Client, region: &str) -> Self {
        Self {
            client,
            region: region.to_string(),
        }
    }

    /// Build a client from fully resolved settings. Credentials kept explicit:
    /// static credentials (from CLI or config) and named profiles replace the
    /// standard credential chain; otherwise the chain (env vars, shared config
    /// file, IAM roles, ...) is used as-is.
    pub async fn new(
        region: &str,
        endpoint: Option<&str>,
        profile: Option<&str>,
        force_path_style: bool,
        credentials: Option<&StaticCredentials>,
    ) -> Result<Self> {
        let mut loader = aws_config::from_env();
        if let Some(profile) = profile.filter(|p| !p.is_empty()) {
            loader = loader.profile_name(profile);
        }
        let base = loader.load().await;

        let mut s3_builder = aws_sdk_s3::config::Builder::from(&base);
        s3_builder = s3_builder.region(aws_config::Region::new(region.to_string()));

        if let Some(endpoint) = endpoint.filter(|e| !e.is_empty()) {
            s3_builder = s3_builder
                .endpoint_url(endpoint)
                .force_path_style(force_path_style);
        }

        if let (Some(ak), Some(sk)) = (
            credentials.and_then(|c| c.access_key_id.as_deref()),
            credentials.and_then(|c| c.secret_access_key.as_deref()),
        ) {
            let session_token = credentials
                .and_then(|c| c.session_token.as_deref())
                .map(str::to_string);
            s3_builder = s3_builder.credentials_provider(aws_sdk_s3::config::Credentials::new(
                ak,
                sk,
                session_token,
                None,
                "s3-tui",
            ));
        }

        let client = Client::from_conf(s3_builder.build());
        Ok(Self {
            client,
            region: region.to_string(),
        })
    }

    pub async fn list_buckets(&self) -> Result<Vec<BucketInfo>> {
        let response = self
            .client
            .list_buckets()
            .send()
            .await
            .context("error al listar los buckets de S3")?;

        let buckets = response
            .buckets()
            .iter()
            .map(|b| BucketInfo {
                name: b.name().unwrap_or("unknown").to_string(),
                creation_date: b.creation_date().and_then(|d| {
                    let secs = d.secs();
                    let nanos = d.subsec_nanos();
                    DateTime::from_timestamp(secs, nanos)
                }),
                region: self.region.clone(),
            })
            .collect();

        Ok(buckets)
    }

    pub async fn list_objects(&self, bucket: &str, prefix: &str) -> Result<Vec<ObjectInfo>> {
        let full_prefix = if prefix.is_empty() {
            String::new()
        } else if prefix.ends_with('/') {
            prefix.to_string()
        } else {
            format!("{prefix}/")
        };

        let response = self
            .client
            .list_objects_v2()
            .bucket(bucket)
            .prefix(&full_prefix)
            .delimiter("/")
            .send()
            .await
            .with_context(|| format!("error al listar objetos en s3://{bucket}/{prefix}"))?;

        let mut objects = Vec::new();

        // Add folders (common prefixes)
        for p in response.common_prefixes() {
            if let Some(path) = p.prefix() {
                let name = path
                    .strip_prefix(&full_prefix)
                    .unwrap_or(path)
                    .trim_end_matches('/');
                if !name.is_empty() {
                    objects.push(ObjectInfo {
                        key: path.to_string(),
                        size: None,
                        last_modified: None,
                        is_folder: true,
                        storage_class: StorageClass::Folder,
                    });
                }
            }
        }

        // Add files
        for obj in response.contents() {
            if let Some(key) = obj.key() {
                // Skip the prefix itself if it's a "folder marker"
                if key == full_prefix.as_str() {
                    continue;
                }
                let name = key.strip_prefix(&full_prefix).unwrap_or(key).to_string();
                if !name.is_empty() {
                    let storage_class = obj
                        .storage_class()
                        .map(map_storage_class)
                        .unwrap_or(StorageClass::Standard);
                    objects.push(ObjectInfo {
                        key: key.to_string(),
                        size: obj.size().map(|s| s as u64),
                        last_modified: obj.last_modified().and_then(|d| {
                            let secs = d.secs();
                            let nanos = d.subsec_nanos();
                            DateTime::from_timestamp(secs, nanos)
                        }),
                        is_folder: false,
                        storage_class,
                    });
                }
            }
        }

        Ok(objects)
    }

    pub async fn delete_object(&self, bucket: &str, key: &str) -> Result<()> {
        self.client
            .delete_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
            .with_context(|| format!("error al borrar s3://{bucket}/{key}"))?;
        Ok(())
    }

    /// Delete several objects in a single S3 batch request. Returns the deleted
    /// keys; any per-object error aborts with a message naming the failures.
    pub async fn delete_objects(&self, bucket: &str, keys: &[String]) -> Result<Vec<String>> {
        use aws_sdk_s3::types::{Delete, ObjectIdentifier};

        let mut objects = Vec::with_capacity(keys.len());
        for key in keys {
            let id = ObjectIdentifier::builder()
                .key(key)
                .build()
                .with_context(|| format!("invalid object key {key}"))?;
            objects.push(id);
        }
        let delete = Delete::builder()
            .set_objects(Some(objects))
            .build()
            .with_context(|| "error al armar la solicitud de borrado")?;
        let resp = self
            .client
            .delete_objects()
            .bucket(bucket)
            .delete(delete)
            .send()
            .await
            .with_context(|| format!("error en el borrado por lotes en s3://{bucket}/"))?;
        let errors = resp.errors();
        if !errors.is_empty() {
            let msg = errors
                .iter()
                .map(|e| {
                    format!(
                        "{}: {}",
                        e.key().unwrap_or("?"),
                        e.message().unwrap_or("error desconocido")
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            return Err(anyhow::anyhow!("el borrado por lotes falló: {msg}"));
        }
        Ok(keys.to_vec())
    }

    /// Upload a single local file into `prefix` at the key `prefix + key_suffix`,
    /// attaching `metadata` when present. Returns the full S3 key.
    pub async fn upload_file(
        &self,
        bucket: &str,
        prefix: &str,
        key_suffix: &str,
        local_path: &str,
        metadata: &[(String, String)],
    ) -> Result<String> {
        let key = format!("{prefix}{key_suffix}");

        let body = aws_sdk_s3::primitives::ByteStream::from_path(local_path)
            .await
            .with_context(|| format!("error al leer el archivo local {local_path}"))?;

        let mut put = self.client.put_object().bucket(bucket).key(&key).body(body);
        if !metadata.is_empty() {
            let map: std::collections::HashMap<String, String> = metadata.iter().cloned().collect();
            put = put.set_metadata(Some(map));
        }
        put.send()
            .await
            .with_context(|| format!("error al subir a s3://{bucket}/{key}"))?;

        Ok(key)
    }

    /// Upload several local files (or whole directory trees) into `prefix`,
    /// keying each by its file name — or by its path relative to the selected
    /// directory when the selection is a folder — and attaching its per-file
    /// metadata. Returns a per-file report so partial failures don't abort the
    /// batch.
    pub async fn upload_files(
        &self,
        bucket: &str,
        prefix: &str,
        files: &[(std::path::PathBuf, Vec<(String, String)>)],
    ) -> UploadReport {
        let mut report = UploadReport::default();
        for (path, key_suffix, metadata) in expand_upload_sources(files) {
            let path_str = path.to_string_lossy();
            match self
                .upload_file(bucket, prefix, &key_suffix, &path_str, &metadata)
                .await
            {
                Ok(key) => report.uploaded.push(format!("{bucket}/{key}")),
                Err(e) => report
                    .failures
                    .push((path.display().to_string(), e.to_string())),
            }
        }
        report
    }

    pub async fn download_file(&self, bucket: &str, key: &str, dest_path: &str) -> Result<()> {
        use aws_sdk_s3::primitives::ByteStream;

        let body = self
            .client
            .get_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
            .with_context(|| format!("error al descargar s3://{bucket}/{key}"))?
            .body;

        let bytes = ByteStream::collect(body).await?;
        let bytes = bytes.into_bytes();

        tokio::fs::write(dest_path, bytes)
            .await
            .with_context(|| format!("error al escribir el archivo local {dest_path}"))?;

        Ok(())
    }

    /// Download several objects into `dest_dir`, preserving each object's file
    /// name. Returns a report with per-object results so partial failures don't
    /// abort the whole batch.
    pub async fn download_objects(
        &self,
        bucket: &str,
        keys: &[String],
        dest_dir: &str,
    ) -> DownloadReport {
        let base = dest_dir.trim_end_matches('/');
        let mut report = DownloadReport::default();

        for key in keys {
            let file_name = std::path::Path::new(key)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("download");
            let dest_path = format!("{base}/{file_name}");

            match self.download_file(bucket, key, &dest_path).await {
                Ok(()) => report.downloaded.push(key.clone()),
                Err(e) => report.failures.push((key.clone(), e.to_string())),
            }
        }

        report
    }

    pub async fn get_object_info(&self, bucket: &str, key: &str) -> Result<ObjectDetail> {
        let response = self
            .client
            .head_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
            .with_context(|| format!("error al consultar s3://{bucket}/{key}"))?;

        let etag = response.e_tag().map(|s| s.to_string());
        let content_type = response.content_type().map(|s| s.to_string());

        let mut metadata: Vec<(String, String)> = response
            .metadata()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        metadata.sort_by(|a, b| a.0.cmp(&b.0));

        Ok(ObjectDetail {
            bucket: bucket.to_string(),
            key: key.to_string(),
            size: response.content_length().map(|l| l as u64),
            last_modified: response.last_modified().and_then(|d| {
                let secs = d.secs();
                let nanos = d.subsec_nanos();
                DateTime::from_timestamp(secs, nanos)
            }),
            storage_class: response
                .storage_class()
                .map(map_head_storage_class)
                .unwrap_or(StorageClass::Standard),
            etag,
            content_type,
            content_length: response.content_length().map(|l| l as u64),
            metadata,
        })
    }
}

fn map_head_storage_class(sc: &aws_sdk_s3::types::StorageClass) -> StorageClass {
    use aws_sdk_s3::types::StorageClass as Aws;
    match sc {
        Aws::Standard => StorageClass::Standard,
        Aws::ReducedRedundancy => StorageClass::ReducedRedundancy,
        Aws::IntelligentTiering => StorageClass::IntelligentTiering,
        Aws::Glacier => StorageClass::Glacier,
        Aws::GlacierIr => StorageClass::GlacierIr,
        Aws::StandardIa => StorageClass::StandardIa,
        Aws::OnezoneIa => StorageClass::OneZoneIa,
        Aws::ExpressOnezone => StorageClass::ExpressOnezone,
        Aws::Outposts => StorageClass::Outposts,
        Aws::DeepArchive => StorageClass::DeepArchive,
        Aws::Snow => StorageClass::Snow,
        other => StorageClass::Unknown(other.as_str().to_string()),
    }
}

/// Statically walk `root`, collecting every file (recursing into subdirectories)
/// as `(local_path, key)` where `key` is the path relative to the selected
/// top-level directory. Symlinks are treated as files. The whole subtree is
/// accumulated into `out` and sorted by key for a deterministic order.
fn collect_dir_files(
    root: &std::path::Path,
    base_key: &str,
    out: &mut Vec<(std::path::PathBuf, String)>,
) {
    let Ok(rd) = std::fs::read_dir(root) else {
        return;
    };
    let mut acc: Vec<(std::path::PathBuf, String)> = Vec::new();
    for entry in rd.flatten() {
        let path = entry.path();
        let file_type = entry.file_type().ok().is_some_and(|ft| ft.is_dir());
        let name = entry.file_name().to_string_lossy().replace('\\', "/");
        let key = format!("{base_key}{name}");
        if file_type {
            collect_dir_files(&path, &format!("{key}/"), &mut acc);
        } else {
            acc.push((path, key));
        }
    }
    acc.sort_by(|a, b| a.1.cmp(&b.1));
    out.extend(acc);
}

/// A single upload unit: (local path, S3 key suffix, per-file metadata).
pub type UploadSource = (std::path::PathBuf, String, Vec<(String, String)>);

// Flatten a selection of local paths into the `(local_path, key, metadata)`
// units the uploader consumes.
//
// - A plain file keeps its file name as the key suffix.
// - A directory is walked recursively; each file under it keeps the path
//   relative to the selected folder (`folder/a/b.txt`), inheriting the
//   metadata of the directory entry. Empty directories contribute nothing.
//
// The order is: selection order first, then each directory's files sorted by
// key for a deterministic listing.
pub fn expand_upload_sources(
    selections: &[(std::path::PathBuf, Vec<(String, String)>)],
) -> Vec<UploadSource> {
    let mut out: Vec<UploadSource> = Vec::new();
    for (path, metadata) in selections {
        if path.is_dir() {
            let dir_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("upload");
            let mut files: Vec<(std::path::PathBuf, String)> = Vec::new();
            collect_dir_files(
                path,
                &format!("{}/", dir_name.replace('\\', "/")),
                &mut files,
            );
            out.extend(files.into_iter().map(|(p, k)| (p, k, metadata.clone())));
        } else {
            let file_name = path
                .file_name()
                .map(|n| n.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|| "upload".to_string());
            out.push((path.clone(), file_name, metadata.clone()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("s3tui-s3-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn expand_keeps_file_names_and_inherits_metadata() {
        let tmp = temp_dir("flat");
        fs::write(tmp.join("one.txt"), "1").unwrap();

        let expanded = expand_upload_sources(&[(
            tmp.join("one.txt"),
            vec![("env".to_string(), "prod".to_string())],
        )]);

        assert_eq!(expanded.len(), 1);
        assert_eq!(expanded[0].1, "one.txt");
        assert_eq!(expanded[0].2, vec![("env".to_string(), "prod".to_string())]);
    }

    #[test]
    fn expand_recurses_directory_trees_preserving_keys() {
        let tmp = temp_dir("tree");
        fs::create_dir_all(tmp.join("docs/sub/deep")).unwrap();
        fs::write(tmp.join("docs/root.txt"), "r").unwrap();
        fs::write(tmp.join("docs/sub/nested.log"), "n").unwrap();
        fs::write(tmp.join("docs/sub/deep/inner.txt"), "i").unwrap();

        let expanded = expand_upload_sources(&[(
            tmp.join("docs"),
            vec![("team".to_string(), "infra".to_string())],
        )]);

        let keys: Vec<&str> = expanded.iter().map(|(_, k, _)| k.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                "docs/root.txt",
                "docs/sub/deep/inner.txt",
                "docs/sub/nested.log"
            ],
            "directory order: sorted by key, relative to the selected folder"
        );
        assert!(
            expanded
                .iter()
                .all(|(_, _, m)| m == &vec![("team".to_string(), "infra".to_string())])
        );
    }

    #[test]
    fn expand_skips_empty_directories() {
        let tmp = temp_dir("empty");
        fs::create_dir_all(tmp.join("solo")).unwrap();

        let expanded = expand_upload_sources(&[(tmp.join("solo"), vec![])]);
        assert!(expanded.is_empty(), "empty folders upload nothing");
    }
}
