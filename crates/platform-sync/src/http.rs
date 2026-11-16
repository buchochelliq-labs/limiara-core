//! The workspace API over HTTP, with the device's own access token
//! (ADR 0029).

use crate::transport::{
    ChangesPage, CompositeResult, EventStream, RecordsPage, StreamEvent, SyncTransport,
};
use crate::wire::{change_entry, changes_page, problem_code};
use crate::SyncError;
use async_trait::async_trait;
use platform_objects::composite::CompositeRequest;
use reqwest::{Method, StatusCode};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

/// How long a stream request may stay open; the server ends streams after
/// 15 minutes.
const STREAM_LIFETIME: Duration = Duration::from_secs(20 * 60);
/// The server sends a keep-alive comment every 15 seconds; a stream silent
/// for longer than this has dropped.
const STREAM_IDLE: Duration = Duration::from_secs(60);

/// Hands the transport the device's current access token. The app keeps
/// the refresh token and rotates it (ADR 0029); after
/// [`SyncError::Unauthorized`] it refreshes and syncs again.
#[async_trait]
pub trait AccessToken: Send + Sync {
    async fn access_token(&self) -> Result<String, SyncError>;
}

/// Calls `{base}/api/v1/workspaces/{workspace}/…`.
pub struct HttpTransport {
    client: reqwest::Client,
    base: String,
    workspace: Uuid,
    token: Arc<dyn AccessToken>,
}

impl HttpTransport {
    /// `base` is the API's origin, such as `https://api.example.com`.
    pub fn new(
        base: impl Into<String>,
        workspace: Uuid,
        token: Arc<dyn AccessToken>,
    ) -> Result<Self, SyncError> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|error| SyncError::Transport(error.to_string()))?;
        Ok(Self::with_client(client, base, workspace, token))
    }

    pub fn with_client(
        client: reqwest::Client,
        base: impl Into<String>,
        workspace: Uuid,
        token: Arc<dyn AccessToken>,
    ) -> Self {
        Self {
            client,
            base: base.into().trim_end_matches('/').to_owned(),
            workspace,
            token,
        }
    }

    /// The API origin this transport calls.
    pub fn base(&self) -> &str {
        &self.base
    }

    fn url(&self, path: &str) -> String {
        format!("{}/api/v1/workspaces/{}{path}", self.base, self.workspace)
    }

    /// Sends a request and returns the status and JSON body of any answer
    /// the caller handles itself; refused tokens become their errors.
    async fn call(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<(StatusCode, Value), SyncError> {
        let token = self.token.access_token().await?;
        let mut request = self
            .client
            .request(method, self.url(path))
            .bearer_auth(token)
            .query(query);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request
            .send()
            .await
            .map_err(|error| SyncError::Transport(error.to_string()))?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|error| SyncError::Transport(error.to_string()))?;
        let body: Value = match serde_json::from_slice(&bytes) {
            Ok(body) => body,
            Err(_) if bytes.is_empty() || !status.is_success() => Value::Null,
            Err(error) => return Err(SyncError::protocol(format!("{status} body: {error}"))),
        };
        if status == StatusCode::UNAUTHORIZED {
            return Err(if problem_code(&body) == "auth.device_wipe_required" {
                SyncError::WipeRequired
            } else {
                SyncError::Unauthorized
            });
        }
        Ok((status, body))
    }

    /// A call whose only expected answer is `200`.
    async fn ok(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<Value, SyncError> {
        let (status, body) = self.call(method, path, query, body).await?;
        if status.is_success() {
            Ok(body)
        } else {
            Err(server(status, &body))
        }
    }
}

fn server(status: StatusCode, body: &Value) -> SyncError {
    SyncError::Server {
        status: status.as_u16(),
        code: problem_code(body),
    }
}

/// Path segments are API names (`custom:deal`) and public ids, which hold
/// only letters, digits, `_` and `:`; anything else is escaped, so a value
/// can never name another route.
fn segment(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b':' => char::from(byte).to_string(),
            other => format!("%{other:02X}"),
        })
        .collect()
}

#[derive(Deserialize)]
struct Data {
    data: Vec<Value>,
    #[serde(default)]
    next_cursor: Option<String>,
}

#[derive(Deserialize)]
struct CompositeBody {
    results: Vec<ResultBody>,
}

#[derive(Deserialize)]
struct ResultBody {
    reference: String,
    status: u16,
    #[serde(default)]
    body: Value,
}

#[derive(Deserialize)]
struct Revoked {
    revoked: BTreeMap<String, Vec<String>>,
}

#[derive(Deserialize)]
struct Named {
    id: Uuid,
    name: String,
}

/// `{"data": [{"id", "name", ...}]}` as names by id.
fn named(value: Value, what: &str) -> Result<BTreeMap<Uuid, String>, SyncError> {
    #[derive(Deserialize)]
    struct Page {
        data: Vec<Named>,
    }
    Ok(parse::<Page>(value, what)?
        .data
        .into_iter()
        .map(|entry| (entry.id, entry.name))
        .collect())
}

fn parse<T: for<'de> Deserialize<'de>>(value: Value, what: &str) -> Result<T, SyncError> {
    serde_json::from_value(value).map_err(|error| SyncError::protocol(format!("{what}: {error}")))
}

#[async_trait]
impl SyncTransport for HttpTransport {
    async fn objects(&self) -> Result<Vec<Value>, SyncError> {
        let body = self.ok(Method::GET, "/objects", &[], None).await?;
        Ok(parse::<Data>(body, "an object list")?.data)
    }

    async fn describe(&self, api_name: &str) -> Result<Option<Value>, SyncError> {
        let (status, body) = self
            .call(
                Method::GET,
                &format!("/objects/{}", segment(api_name)),
                &[],
                None,
            )
            .await?;
        match status {
            StatusCode::NOT_FOUND => Ok(None),
            status if status.is_success() => Ok(Some(body)),
            status => Err(server(status, &body)),
        }
    }

    async fn validation_rules(&self, api_name: &str) -> Result<Option<Vec<Value>>, SyncError> {
        let (status, body) = self
            .call(
                Method::GET,
                &format!("/objects/{}/validation-rules", segment(api_name)),
                &[],
                None,
            )
            .await?;
        match status {
            StatusCode::FORBIDDEN | StatusCode::NOT_FOUND => Ok(None),
            status if status.is_success() => Ok(Some(parse::<Data>(body, "a rule list")?.data)),
            status => Err(server(status, &body)),
        }
    }

    async fn quick_actions(&self, api_name: &str) -> Result<Option<Vec<Value>>, SyncError> {
        let (status, body) = self
            .call(
                Method::GET,
                &format!("/objects/{}/quick-actions", segment(api_name)),
                &[],
                None,
            )
            .await?;
        match status {
            StatusCode::FORBIDDEN | StatusCode::NOT_FOUND => Ok(None),
            status if status.is_success() => {
                Ok(Some(parse::<Data>(body, "a quick action list")?.data))
            }
            status => Err(server(status, &body)),
        }
    }

    async fn composite(
        &self,
        request: &CompositeRequest,
    ) -> Result<Vec<CompositeResult>, SyncError> {
        let request = serde_json::to_value(request)
            .map_err(|error| SyncError::protocol(format!("a composite request: {error}")))?;
        let body = self
            .ok(Method::POST, "/composite", &[], Some(&request))
            .await?;
        Ok(parse::<CompositeBody>(body, "a composite response")?
            .results
            .into_iter()
            .map(|result| CompositeResult {
                reference: result.reference,
                status: result.status,
                body: result.body,
            })
            .collect())
    }

    async fn record(&self, api_name: &str, public_id: &str) -> Result<Option<Value>, SyncError> {
        let (status, body) = self
            .call(
                Method::GET,
                &format!(
                    "/objects/{}/records/{}",
                    segment(api_name),
                    segment(public_id)
                ),
                &[("include_deleted", "true".to_owned())],
                None,
            )
            .await?;
        match status {
            StatusCode::NOT_FOUND | StatusCode::FORBIDDEN => Ok(None),
            status if status.is_success() => Ok(Some(body)),
            status => Err(server(status, &body)),
        }
    }

    async fn changes(&self, after: Option<i64>, limit: u16) -> Result<ChangesPage, SyncError> {
        let mut query = vec![("limit", limit.to_string())];
        if let Some(after) = after {
            query.push(("after", after.to_string()));
        }
        let (status, body) = self
            .call(Method::GET, "/objects/changes", &query, None)
            .await?;
        if status == StatusCode::GONE && problem_code(&body) == "changes.position_expired" {
            return Err(SyncError::PositionExpired);
        }
        if !status.is_success() {
            return Err(server(status, &body));
        }
        changes_page(&body)
    }

    async fn stream(&self, after: Option<i64>) -> Result<EventStream, SyncError> {
        use futures_util::StreamExt;
        let token = self.token.access_token().await?;
        let mut request = self
            .client
            .get(self.url("/objects/changes/stream"))
            .bearer_auth(token)
            .header("accept", "text/event-stream")
            .timeout(STREAM_LIFETIME);
        if let Some(after) = after {
            request = request.query(&[("after", after.to_string())]);
        }
        let response = request
            .send()
            .await
            .map_err(|error| SyncError::Transport(error.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            let bytes = response.bytes().await.unwrap_or_default();
            let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            return Err(match (status, problem_code(&body).as_str()) {
                (StatusCode::UNAUTHORIZED, "auth.device_wipe_required") => SyncError::WipeRequired,
                (StatusCode::UNAUTHORIZED, _) => SyncError::Unauthorized,
                (StatusCode::GONE, "changes.position_expired") => SyncError::PositionExpired,
                _ => server(status, &body),
            });
        }
        struct Reading {
            bytes: futures_util::stream::BoxStream<'static, reqwest::Result<Vec<u8>>>,
            buffer: String,
            ready: std::collections::VecDeque<Result<StreamEvent, SyncError>>,
            ended: bool,
        }
        let reading = Reading {
            bytes: response
                .bytes_stream()
                .map(|chunk| chunk.map(|chunk| chunk.to_vec()))
                .boxed(),
            buffer: String::new(),
            ready: std::collections::VecDeque::new(),
            ended: false,
        };
        let events = futures_util::stream::unfold(reading, |mut reading| async move {
            loop {
                if let Some(event) = reading.ready.pop_front() {
                    return Some((event, reading));
                }
                if reading.ended {
                    return None;
                }
                match tokio::time::timeout(STREAM_IDLE, reading.bytes.next()).await {
                    Err(_) => {
                        reading.ended = true;
                        reading
                            .ready
                            .push_back(Err(SyncError::Transport("the stream went quiet".into())));
                    }
                    Ok(None) => reading.ended = true,
                    Ok(Some(Err(error))) => {
                        reading.ended = true;
                        reading
                            .ready
                            .push_back(Err(SyncError::Transport(error.to_string())));
                    }
                    Ok(Some(Ok(chunk))) => {
                        reading.buffer.push_str(&String::from_utf8_lossy(&chunk));
                        for event in take_events(&mut reading.buffer) {
                            if let Some(event) = stream_event(&event) {
                                reading.ready.push_back(event);
                            }
                        }
                    }
                }
            }
        });
        Ok(events.boxed())
    }

    async fn records_page(
        &self,
        api_name: &str,
        cursor: Option<&str>,
    ) -> Result<RecordsPage, SyncError> {
        let mut query = vec![
            ("status", "any".to_owned()),
            ("sort", "touched_at".to_owned()),
            ("direction", "asc".to_owned()),
            ("limit", "200".to_owned()),
        ];
        if let Some(cursor) = cursor {
            query.push(("cursor", cursor.to_owned()));
        }
        let body = self
            .ok(
                Method::GET,
                &format!("/objects/{}/records", segment(api_name)),
                &query,
                None,
            )
            .await?;
        let page = parse::<Data>(body, "a record page")?;
        Ok(RecordsPage {
            records: page.data,
            next_cursor: page.next_cursor,
        })
    }

    async fn directory(&self) -> Result<platform_objects::ReplicaDirectory, SyncError> {
        let members = self.ok(Method::GET, "/users", &[], None).await?;
        let roles = self.ok(Method::GET, "/roles", &[], None).await?;
        Ok(platform_objects::ReplicaDirectory {
            members: named(members, "a member list")?,
            roles: named(roles, "a role list")?,
        })
    }

    async fn revocations(
        &self,
        records: &BTreeMap<String, Vec<String>>,
    ) -> Result<BTreeMap<String, Vec<String>>, SyncError> {
        let body = self
            .ok(
                Method::POST,
                "/sync/revocations",
                &[],
                Some(&json!({ "records": records })),
            )
            .await?;
        Ok(parse::<Revoked>(body, "a revocation set")?.revoked)
    }
}

/// One server-sent event: its `event` name and `data`.
#[derive(Debug, Default, PartialEq)]
struct SseEvent {
    event: String,
    data: String,
}

/// Takes the complete events from the front of `buffer`, leaving any
/// partial one for the next chunk. Comments (keep-alives) are dropped.
fn take_events(buffer: &mut String) -> Vec<SseEvent> {
    if buffer.contains('\r') {
        // A carriage return at the very end may be the first half of a CRLF
        // split across chunks: it waits for the next chunk.
        let held = buffer.ends_with('\r');
        let end = buffer.len() - usize::from(held);
        let mut normalised = buffer[..end].replace("\r\n", "\n").replace('\r', "\n");
        if held {
            normalised.push('\r');
        }
        *buffer = normalised;
    }
    let mut events = Vec::new();
    while let Some(end) = buffer.find("\n\n") {
        let block: String = buffer.drain(..end + 2).collect();
        let mut event = SseEvent::default();
        let mut data = Vec::new();
        for line in block.lines() {
            if line.is_empty() || line.starts_with(':') {
                continue;
            }
            let (field, value) = line.split_once(':').unwrap_or((line, ""));
            let value = value.strip_prefix(' ').unwrap_or(value);
            match field {
                "event" => event.event = value.to_owned(),
                "data" => data.push(value),
                _ => {}
            }
        }
        if !data.is_empty() || !event.event.is_empty() {
            event.data = data.join("\n");
            events.push(event);
        }
    }
    events
}

/// The sync meaning of a server-sent event; events the loop does not use
/// are passed over.
fn stream_event(event: &SseEvent) -> Option<Result<StreamEvent, SyncError>> {
    let data = || {
        serde_json::from_str::<Value>(&event.data)
            .map_err(|error| SyncError::protocol(format!("a stream event: {error}")))
    };
    match event.event.as_str() {
        "change" => Some(data().and_then(|value| change_entry(&value).map(StreamEvent::Change))),
        "metadata" => Some(data().and_then(|value| {
            value
                .get("metadata_version")
                .and_then(Value::as_i64)
                .map(StreamEvent::Metadata)
                .ok_or_else(|| SyncError::protocol("a metadata event has `metadata_version`"))
        })),
        "expired" => Some(Ok(StreamEvent::Expired)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::ChangeEntry;

    #[test]
    fn events_are_taken_whole_across_chunks() {
        let mut buffer = String::from(": keep-alive\n\nevent: metadata\ndata: {\"metadata_version\": 3}\n\nevent: change\nid: 7\ndata: {\"position\": 7,");
        let events = take_events(&mut buffer);
        assert_eq!(
            events,
            vec![SseEvent {
                event: "metadata".into(),
                data: "{\"metadata_version\": 3}".into()
            }]
        );
        assert_eq!(
            stream_event(&events[0]).expect("used").expect("valid"),
            StreamEvent::Metadata(3)
        );
        buffer.push_str(" \"object\": \"custom:deal\", \"record\": \"a0B1\", \"kind\": \"updated\"}\r\n\r\nevent: expired\ndata: {}\n\n");
        let events = take_events(&mut buffer);
        assert_eq!(events.len(), 2);
        assert!(matches!(
            stream_event(&events[0]),
            Some(Ok(StreamEvent::Change(ChangeEntry { position: 7, .. })))
        ));
        assert_eq!(
            stream_event(&events[1]).expect("used").expect("valid"),
            StreamEvent::Expired
        );
        assert!(buffer.is_empty());

        // A CRLF split across chunks is one line ending.
        let mut buffer = String::from("event: metadata\r");
        assert!(take_events(&mut buffer).is_empty());
        buffer.push_str("\ndata: {\"metadata_version\": 5}\r");
        assert!(take_events(&mut buffer).is_empty());
        buffer.push_str("\n\r\n");
        let events = take_events(&mut buffer);
        assert_eq!(
            events,
            vec![SseEvent {
                event: "metadata".into(),
                data: "{\"metadata_version\": 5}".into()
            }]
        );
        assert!(buffer.is_empty());
        assert!(stream_event(&SseEvent {
            event: "other".into(),
            data: String::new()
        })
        .is_none());
    }

    #[test]
    fn path_segments_keep_api_names_and_escape_the_rest() {
        assert_eq!(segment("custom:deal"), "custom:deal");
        assert_eq!(segment("a0B000000000001AAA"), "a0B000000000001AAA");
        assert_eq!(segment("../x y"), "%2E%2E%2Fx%20y");
    }
}
