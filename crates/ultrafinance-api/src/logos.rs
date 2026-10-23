//! Serve immutable local logo files without consulting the catalog.
use axum::{
    Router,
    extract::Path,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use std::path::PathBuf;

pub fn router<S: Clone + Send + Sync + 'static>(directory: PathBuf) -> Router<S> {
    Router::new().route(
        "/logos/assets/{filename}",
        get(move |Path(filename): Path<String>| serve(directory.clone(), filename)),
    )
}

async fn serve(directory: PathBuf, filename: String) -> Response {
    let Some((hash, extension)) = filename.split_once('.') else {
        return StatusCode::NOT_FOUND.into_response();
    };
    // Only content-addressed image keys are valid; never accept filesystem paths.
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    let mime = match extension {
        "webp" => "image/webp",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "svg" => "image/svg+xml",
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let result =
        tokio::task::spawn_blocking(move || std::fs::read(directory.join("assets").join(filename)))
            .await;
    match result {
        Ok(Ok(bytes)) => (
            [
                (header::CONTENT_TYPE, mime),
                (header::CACHE_CONTROL, "public,max-age=31536000,immutable"),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
                (
                    header::CONTENT_SECURITY_POLICY,
                    "default-src 'none'; sandbox",
                ),
                (
                    header::CONTENT_DISPOSITION,
                    if mime == "image/svg+xml" {
                        "attachment"
                    } else {
                        "inline"
                    },
                ),
            ],
            bytes,
        )
            .into_response(),
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            StatusCode::NOT_FOUND.into_response()
        }
        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;

    #[tokio::test]
    async fn serves_local_assets_with_safe_headers_and_rejects_other_paths() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("assets")).unwrap();
        let hash = "a".repeat(64);
        for extension in ["webp", "svg"] {
            std::fs::write(
                directory.path().join(format!("assets/{hash}.{extension}")),
                b"logo",
            )
            .unwrap();
        }
        let app: Router = router(directory.path().into());
        for (extension, mime, disposition) in [
            ("webp", "image/webp", "inline"),
            ("svg", "image/svg+xml", "attachment"),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::get(format!("/logos/assets/{hash}.{extension}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[header::CONTENT_TYPE], mime);
            assert_eq!(response.headers()[header::CONTENT_DISPOSITION], disposition);
            assert_eq!(
                response.headers()[header::X_CONTENT_TYPE_OPTIONS],
                "nosniff"
            );
            assert!(
                response.headers()[header::CONTENT_SECURITY_POLICY]
                    .to_str()
                    .unwrap()
                    .contains("sandbox")
            );
            assert!(
                response.headers()[header::CACHE_CONTROL]
                    .to_str()
                    .unwrap()
                    .contains("immutable")
            );
            assert_eq!(to_bytes(response.into_body(), 1024).await.unwrap(), "logo");
        }
        for path in [
            "missing.webp".to_owned(),
            format!("{hash}.html"),
            format!("{}.png", "b".repeat(64)),
            "..%2F..%2F.env".to_owned(),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::get(format!("/logos/assets/{path}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }
    }
}
