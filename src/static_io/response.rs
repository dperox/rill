use crate::error_page;
use axum::{
    body::Body,
    extract::Request,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use std::{path::PathBuf, sync::Arc, time::SystemTime};
use tower::{Service, ServiceExt};
use tower_http::services::ServeDir;

pub(super) fn static_etag(seconds: u64, length: u64) -> header::HeaderValue {
    format!("\"{seconds:x}-{length:x}\"").parse().unwrap()
}

fn response_length(headers: &HeaderMap) -> Option<u64> {
    let length = match headers.get(header::CONTENT_RANGE) {
        Some(range) => range.to_str().ok()?.rsplit_once('/')?.1,
        None => headers.get(header::CONTENT_LENGTH)?.to_str().ok()?,
    };
    length.parse().ok()
}

pub(crate) fn has_repeated_condition(headers: &HeaderMap) -> bool {
    headers.len() != headers.keys_len()
        && [
            header::IF_MATCH,
            header::IF_NONE_MATCH,
            header::IF_MODIFIED_SINCE,
            header::IF_UNMODIFIED_SINCE,
            header::IF_RANGE,
        ]
        .iter()
        .any(|name| headers.get_all(name).iter().nth(1).is_some())
}

#[derive(Clone, PartialEq)]
pub(super) struct StaticConditions {
    unmodified_since: Option<header::HeaderValue>,
    if_match: Option<header::HeaderValue>,
    modified_since: Option<header::HeaderValue>,
    none_match: Option<header::HeaderValue>,
}

impl StaticConditions {
    fn remove_from(headers: &mut HeaderMap) -> Self {
        Self {
            unmodified_since: headers.remove(header::IF_UNMODIFIED_SINCE),
            if_match: headers.remove(header::IF_MATCH),
            modified_since: headers.remove(header::IF_MODIFIED_SINCE),
            none_match: headers.remove(header::IF_NONE_MATCH),
        }
    }
}

pub(super) fn static_condition_response(
    status: StatusCode,
    last_modified: Option<header::HeaderValue>,
    etag: Option<header::HeaderValue>,
) -> Response {
    if status == StatusCode::NOT_MODIFIED {
        let mut response = status.into_response();
        if let Some(value) = last_modified {
            response.headers_mut().insert(header::LAST_MODIFIED, value);
        }
        if let Some(value) = etag {
            response.headers_mut().insert(header::ETAG, value);
        }
        response
    } else {
        error_page(status)
    }
}

fn etag_matches(
    value: &header::HeaderValue,
    etag: Option<&header::HeaderValue>,
    weak: bool,
) -> bool {
    let value = value.as_bytes();
    if value == b"*" {
        return true;
    }
    let Some(etag) = etag else {
        return false;
    };
    value.split(|byte| *byte == b',').any(|candidate| {
        let candidate = candidate.trim_ascii();
        let candidate = if weak {
            candidate.strip_prefix(b"W/").unwrap_or(candidate)
        } else {
            candidate
        };
        candidate == etag.as_bytes()
    })
}

pub(super) fn static_precondition(
    conditions: &StaticConditions,
    modified: Option<SystemTime>,
    etag: Option<&header::HeaderValue>,
) -> Option<StatusCode> {
    let parse_date =
        |value: &header::HeaderValue| httpdate::parse_http_date(value.to_str().ok()?).ok();
    if conditions
        .if_match
        .as_ref()
        .is_some_and(|value| !etag_matches(value, etag, false))
        || (conditions.if_match.is_none()
            && conditions.unmodified_since.as_ref().is_some_and(|value| {
                matches!((modified, parse_date(value)), (Some(modified), Some(date)) if date < modified)
            }))
    {
        return Some(StatusCode::PRECONDITION_FAILED);
    }

    if let Some(none_match) = conditions.none_match.as_ref() {
        if etag_matches(none_match, etag, true) {
            return Some(StatusCode::NOT_MODIFIED);
        }
    } else if let Some(since) = conditions.modified_since.as_ref()
        && modified.is_some_and(|date| Some(date) == parse_date(since))
    {
        return Some(StatusCode::NOT_MODIFIED);
    }

    None
}

fn file_validators(headers: &HeaderMap) -> (Option<SystemTime>, Option<header::HeaderValue>) {
    let last_modified = headers.get(header::LAST_MODIFIED);
    let modified = last_modified
        .and_then(|value| value.to_str().ok())
        .and_then(|date| httpdate::parse_http_date(date).ok());
    let etag = modified
        .and_then(|date| date.duration_since(SystemTime::UNIX_EPOCH).ok())
        .zip(response_length(headers))
        .map(|(elapsed, length)| static_etag(elapsed.as_secs(), length));

    (modified, etag)
}

async fn serve_dir(mut files: ServeDir, mut req: Request) -> Response {
    if req.headers().contains_key(header::RANGE)
        && let Some(validator) = req.headers_mut().remove(header::IF_RANGE)
    {
        let (mut parts, body) = req.into_parts();
        let ranged = files
            .call(Request::from_parts(parts.clone(), body))
            .await
            .unwrap()
            .into_response();
        if !matches!(
            ranged.status(),
            StatusCode::PARTIAL_CONTENT | StatusCode::RANGE_NOT_SATISFIABLE
        ) {
            return ranged;
        }
        // Check the file actually opened for this range, including an unsatisfiable range.
        let (modified, etag) = file_validators(ranged.headers());
        let date = validator
            .to_str()
            .ok()
            .and_then(|s| httpdate::parse_http_date(s).ok());
        if etag.as_ref() == Some(&validator) || modified.is_some_and(|mtime| Some(mtime) == date) {
            return ranged;
        }
        parts.headers.remove(header::RANGE);
        req = Request::from_parts(parts, Body::empty());
    }
    files.oneshot(req).await.unwrap().into_response()
}

pub(crate) async fn serve_static(
    root: Arc<PathBuf>,
    files: ServeDir,
    stream_chunk: usize,
    cache: Option<Arc<super::AssetCache>>,
    mut req: Request,
) -> Response {
    let conditions = StaticConditions::remove_from(req.headers_mut());

    if let Some(task) = super::ordinary_file(
        root.clone(),
        stream_chunk,
        &req,
        &conditions,
        cache.as_ref(),
    ) && let Ok(Ok(outcome)) = task.await
    {
        match outcome {
            super::OrdinaryOutcome::Response(response) => return response,
            super::OrdinaryOutcome::Reject(status) => return error_page(status),
            super::OrdinaryOutcome::Fallback => {}
        }
    }

    let check_root = root;
    let check_path = req.uri().path().to_owned();
    let safe =
        tokio::task::spawn_blocking(move || super::check_fallback_path(&check_root, &check_path))
            .await
            .unwrap_or(false);
    if !safe {
        return error_page(StatusCode::NOT_FOUND);
    }

    // ServeDir retains file metadata on 206 and 416, so preconditions can
    // take precedence over the range result from the same file lookup.
    let mut res = serve_dir(files, req).await;
    match res.status() {
        StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED => return error_page(res.status()),
        StatusCode::OK | StatusCode::PARTIAL_CONTENT | StatusCode::RANGE_NOT_SATISFIABLE => {}
        _ => return res,
    }

    let last_modified = res.headers().get(header::LAST_MODIFIED);
    let (modified, etag) = file_validators(res.headers());

    if let Some(status) = static_precondition(&conditions, modified, etag.as_ref()) {
        return static_condition_response(status, last_modified.cloned(), etag);
    }

    match res.status() {
        StatusCode::RANGE_NOT_SATISFIABLE => {
            let mut error = error_page(res.status());
            if let Some(range) = res.headers_mut().remove(header::CONTENT_RANGE) {
                error.headers_mut().insert(header::CONTENT_RANGE, range);
            }
            return error;
        }
        StatusCode::PARTIAL_CONTENT => {
            res.headers_mut().remove(header::ACCEPT_RANGES);
        }
        _ => {}
    }
    if let Some(value) = etag {
        res.headers_mut().insert(header::ETAG, value);
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn precondition_follows_rfc9110_precedence_and_ignores_invalid_unmodified_since() {
        let modified = Some(UNIX_EPOCH + Duration::from_secs(1_700_000_000));
        let etag = static_etag(1_700_000_000, 20);

        // Invalid If-Unmodified-Since must be ignored, not 412.
        let mut headers = HeaderMap::new();
        headers.insert(
            header::IF_UNMODIFIED_SINCE,
            header::HeaderValue::from_static("not-a-date"),
        );
        let cond = StaticConditions::remove_from(&mut headers);
        assert_eq!(static_precondition(&cond, modified, Some(&etag)), None);

        // Matching If-Match overrides older If-Unmodified-Since.
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_MATCH, etag.clone());
        headers.insert(
            header::IF_UNMODIFIED_SINCE,
            header::HeaderValue::from_static("Tue, 14 Nov 2023 22:13:19 GMT"),
        );
        let cond = StaticConditions::remove_from(&mut headers);
        assert_eq!(static_precondition(&cond, modified, Some(&etag)), None);

        // Matching If-None-Match overrides older/invalid If-Modified-Since.
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, etag.clone());
        headers.insert(
            header::IF_MODIFIED_SINCE,
            header::HeaderValue::from_static("not-a-date"),
        );
        let cond = StaticConditions::remove_from(&mut headers);
        assert_eq!(
            static_precondition(&cond, modified, Some(&etag)),
            Some(StatusCode::NOT_MODIFIED)
        );
    }
}
