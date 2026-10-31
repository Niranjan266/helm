//! `file.upload.begin`, `file.upload.end`, `file.download`.

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{str_param, u64_param};
use crate::files::{self, MAX_FILE_SIZE, MAX_OPEN_UPLOADS, Upload};
use crate::platform::blocking;
use crate::router::{MethodSpec, Module, RequestContext, RpcError, Tier};

pub struct FilesModule;

const fn m(name: &'static str, min_tier: Tier) -> MethodSpec {
    MethodSpec { name, min_tier }
}

const METHODS: &[MethodSpec] = &[
    m("file.upload.begin", Tier::Remote),
    m("file.upload.end", Tier::Remote),
    m("file.download", Tier::Admin),
];

fn parse_sha256(text: &str) -> Result<[u8; 32], RpcError> {
    let bytes = hex::decode(text.trim())
        .map_err(|_| RpcError::bad_params("sha256 must be 64 hex characters"))?;
    bytes
        .try_into()
        .map_err(|_| RpcError::bad_params("sha256 must be 64 hex characters"))
}

#[async_trait]
impl Module for FilesModule {
    fn methods(&self) -> &'static [MethodSpec] {
        METHODS
    }

    async fn call(
        &self,
        method: &str,
        params: Value,
        ctx: &RequestContext,
    ) -> Result<Value, RpcError> {
        let state = ctx.state.clone();
        match method {
            "file.upload.begin" => {
                let name = files::safe_file_name(str_param(&params, "name")?)
                    .map_err(RpcError::bad_params)?;
                let size = u64_param(&params, "size")?;
                if size > MAX_FILE_SIZE {
                    return Err(RpcError::bad_params("file is larger than 4 GiB"));
                }
                let sha256 = parse_sha256(str_param(&params, "sha256")?)?;
                if ctx.session.upload_count() >= MAX_OPEN_UPLOADS {
                    return Err(RpcError::bad_params("too many uploads in progress"));
                }
                let id = files::new_transfer_id();
                let upload = Upload::begin(&state.downloads_dir, &id, name.clone(), size, sha256)
                    .await
                    .map_err(|e| RpcError::internal(format!("cannot create file: {e}")))?;
                ctx.session.add_upload(id, upload);
                state.audit.record(
                    "file_upload",
                    Some(&ctx.device_id),
                    Some(&ctx.peer),
                    &format!("name={name} size={size}"),
                );
                Ok(json!({"transfer_id": hex::encode(id)}))
            }
            "file.upload.end" => {
                let id = files::parse_transfer_id(str_param(&params, "transfer_id")?)
                    .ok_or_else(|| RpcError::bad_params("transfer_id must be 32 hex characters"))?;
                let upload = ctx
                    .session
                    .take_upload(&id)
                    .await
                    .ok_or_else(|| RpcError::bad_params("no such upload"))?;
                let path = upload
                    .finish(&state.downloads_dir)
                    .await
                    .map_err(RpcError::bad_params)?;
                Ok(json!({"path": path.to_string_lossy()}))
            }
            "file.download" => {
                let path = str_param(&params, "path")?.to_string();
                let root = state.download_root.clone();
                let data_dir = state.data_dir.clone();
                let (canonical, size, name, sha) = blocking(move || {
                    let (canonical, size, name) = files::check_download(&path, &root, &data_dir)?;
                    let sha = files::hash_file(&canonical)
                        .map_err(|e| RpcError::internal(format!("cannot read file: {e}")))?;
                    Ok((canonical, size, name, sha))
                })
                .await?;
                let id = files::new_transfer_id();
                state.audit.record(
                    "file_download",
                    Some(&ctx.device_id),
                    Some(&ctx.peer),
                    &format!("path={} size={size}", canonical.display()),
                );
                let session = ctx.session.clone();
                ctx.session.after_response(
                    ctx.req_id,
                    Box::pin(files::stream_download(session, id, canonical, size)),
                );
                Ok(json!({
                    "transfer_id": hex::encode(id),
                    "size": size,
                    "sha256": hex::encode(sha),
                    "name": name,
                }))
            }
            _ => Err(RpcError::unknown_method(method)),
        }
    }
}
