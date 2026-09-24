//! Native Chrome DevTools Protocol backend.
//!
//! Attaches to an existing Chromium-family browser over its loopback remote
//! debugging port, selects exactly one page, frame chain, and element, and
//! replaces that element's content with the credential through
//! `Input.insertText`. The protocol surface and the page-side definitions are
//! those proven at parity with Playwright in Phase A (ADR 0001). No target,
//! context, or page is ever created or closed, and closing the connection is
//! the only cleanup.
//!
//! Every protocol await runs under the coordinator's deadline: dropping the
//! operation future stops all protocol activity.

mod scripts;
mod transport;

use std::collections::HashMap;

use serde_json::{json, Value};

use self::transport::{CdpError, Connection};
use super::{Backend, BrowserTarget, Deadline, Delivery, Effect, FillError, PageMatch, Reason};

const WORLD_NAME: &str = "agentenv-credential-fill";
const SUPPORTED_PROTOCOL: &str = "1.3";

#[derive(Clone, Debug)]
struct Child {
    target_id: String,
    session_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PageTarget {
    target_id: String,
    context_index: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FrameRef {
    session_id: String,
    frame_id: String,
}

#[derive(Clone, Debug)]
struct Element {
    frame: FrameRef,
    object_id: String,
}

enum Target {
    Context(i64),
    Object(String),
}

pub struct CdpBackend {
    target: BrowserTarget,
    connection: Option<Connection>,
    page_session: Option<String>,
    children: Vec<Child>,
    worlds: HashMap<(String, String), i64>,
    prepared: Option<(PageTarget, Vec<FrameRef>, Element)>,
}

impl CdpBackend {
    pub fn new(target: BrowserTarget) -> Self {
        Self {
            target,
            connection: None,
            page_session: None,
            children: Vec::new(),
            worlds: HashMap::new(),
            prepared: None,
        }
    }
}

fn fail(reason: Reason, message: impl Into<String>) -> FillError {
    FillError::new(reason, message)
}

fn protocol(error: CdpError, reason: Reason) -> FillError {
    FillError::new(reason, error.message())
}

impl Backend for CdpBackend {
    fn name(&self) -> &'static str {
        "cdp"
    }

    async fn prepare(&mut self, _deadline: &Deadline) -> Result<(), FillError> {
        let (connection, _version) = Connection::open(&self.target.endpoint)
            .await
            .map_err(|error| protocol(error, Reason::ConnectFailed))?;
        self.connection = Some(connection);
        self.check_versions().await?;
        let page = self.select_page().await?;
        self.check_devtools(&page).await?;
        self.attach(&page).await?;
        let chain = self.resolve_chain().await?;
        let element = self.find_element(&chain).await?;
        self.prepared = Some((page, chain, element));
        Ok(())
    }

    async fn deliver(&mut self, delivery: &Delivery) -> Result<Effect, FillError> {
        #[cfg(all(feature = "test-keychain", debug_assertions))]
        test_delay().await;

        let (page, chain, element) = self
            .prepared
            .clone()
            .ok_or_else(|| fail(Reason::TargetChanged, "the destination was not prepared"))?;
        // A DevTools window opened during credential lookup is a conflict
        // like one open at preflight.
        self.check_devtools(&page).await?;
        self.revalidate(&page, &chain, &element).await?;
        let focused = self
            .call(
                &element.frame.session_id,
                Target::Object(element.object_id.clone()),
                scripts::SELECT_AND_FOCUS,
                vec![],
                true,
                Reason::TargetChanged,
            )
            .await?
            .get("value")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !focused {
            return Err(fail(
                Reason::TargetChanged,
                "the target element could not take focus; it may sit inside another editable region, or the page moved focus elsewhere",
            ));
        }
        let page_session = self.page_session.clone().unwrap_or_default();
        let value = delivery.begin_mutation()?;
        self.connection_mut()?
            .send(
                Some(&page_session),
                "Input.insertText",
                json!({ "text": value.as_str() }),
            )
            .await
            .map_err(|error| FillError::uncertain(Reason::DeliveryFailed, error.message()))?;
        Ok(Effect::FieldFilled)
    }

    async fn release(&mut self, _grace: &Deadline) -> Result<(), FillError> {
        self.prepared = None;
        self.worlds.clear();
        self.children.clear();
        self.page_session = None;
        match self.connection.take() {
            Some(connection) => connection
                .close()
                .await
                .map_err(|error| fail(Reason::CleanupUnconfirmed, error.message())),
            None => Ok(()),
        }
    }
}

/// Debug-only race-window hook for the CDP lab: sleeps between preparation
/// and delivery. A delay that outlasts the deadline is cut short by the
/// coordinator dropping this future, so expiry is reported by the
/// production path.
#[cfg(all(feature = "test-keychain", debug_assertions))]
async fn test_delay() {
    if let Some(delay) = std::env::var("AGENTENV_FILL_TEST_DELAY_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(std::time::Duration::from_millis)
    {
        tokio::time::sleep(delay).await;
    }
}

impl CdpBackend {
    fn connection_mut(&mut self) -> Result<&mut Connection, FillError> {
        self.connection
            .as_mut()
            .ok_or_else(|| fail(Reason::ConnectFailed, "the browser connection is not open"))
    }

    async fn send(
        &mut self,
        session_id: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<Value, CdpError> {
        let connection = self
            .connection
            .as_mut()
            .ok_or_else(|| CdpError::closed(method))?;
        connection.send(session_id, method, params).await
    }

    async fn check_versions(&mut self) -> Result<(), FillError> {
        let version = self
            .send(None, "Browser.getVersion", json!({}))
            .await
            .map_err(|error| protocol(error, Reason::ConnectFailed))?;
        let protocol_version = version.get("protocolVersion").and_then(Value::as_str);
        if protocol_version != Some(SUPPORTED_PROTOCOL) {
            return Err(fail(
                Reason::VersionUnsupported,
                format!("the browser does not speak protocol version {SUPPORTED_PROTOCOL}"),
            ));
        }
        Ok(())
    }

    async fn select_page(&mut self) -> Result<PageTarget, FillError> {
        let contexts = self
            .send(None, "Target.getBrowserContexts", json!({}))
            .await
            .map_err(|error| protocol(error, Reason::ConnectFailed))?;
        let context_ids: Vec<String> = contexts
            .get("browserContextIds")
            .and_then(Value::as_array)
            .map(|ids| {
                ids.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        if let Some(index) = self.target.context_index {
            if index > context_ids.len() {
                return Err(fail(
                    Reason::ContextAbsent,
                    format!(
                        "context index {index} does not exist; {} contexts are open",
                        context_ids.len() + 1
                    ),
                ));
            }
        }
        let targets = self
            .send(None, "Target.getTargets", json!({}))
            .await
            .map_err(|error| protocol(error, Reason::ConnectFailed))?;
        let mut matches = Vec::new();
        for info in targets
            .get("targetInfos")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if info.get("type").and_then(Value::as_str) != Some("page") {
                continue;
            }
            let url = info.get("url").and_then(Value::as_str).unwrap_or_default();
            let context_index = info
                .get("browserContextId")
                .and_then(Value::as_str)
                .and_then(|id| context_ids.iter().position(|known| known == id))
                .map_or(0, |position| position + 1);
            if let Some(wanted) = self.target.context_index {
                if wanted != context_index {
                    continue;
                }
            }
            if !url_matches(self.target.page_match, &self.target.page_url, url) {
                continue;
            }
            matches.push(PageTarget {
                target_id: info
                    .get("targetId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                context_index,
            });
        }
        match matches.len() {
            0 => Err(fail(
                Reason::PageAbsent,
                "no open page matches the requested URL; check --page-url or use --page-match origin-path",
            )),
            1 => Ok(matches.remove(0)),
            count => Err(fail(
                Reason::PageAmbiguous,
                format!("{count} open pages match the requested URL; use --context-index or close duplicates"),
            )),
        }
    }

    /// An open DevTools window is the one recorder a second client can
    /// observe; it fails preflight and is rechecked before delivery. A
    /// browser without `Target.getDevToolsTarget` is checked conservatively:
    /// any open `devtools://` target anywhere in the browser is a conflict.
    async fn check_devtools(&mut self, page: &PageTarget) -> Result<(), FillError> {
        let conflict = fail(
            Reason::RecordingConflict,
            "DevTools is open on the target page; close it before filling, because its recorder state cannot be read",
        );
        match self
            .send(
                None,
                "Target.getDevToolsTarget",
                json!({ "targetId": page.target_id }),
            )
            .await
        {
            Ok(result) => {
                if result.get("targetId").and_then(Value::as_str).is_some() {
                    return Err(conflict);
                }
                Ok(())
            }
            Err(error) if error.method_not_found() => {
                let targets = self
                    .send(None, "Target.getTargets", json!({}))
                    .await
                    .map_err(|error| protocol(error, Reason::ConnectFailed))?;
                let devtools_open = targets
                    .get("targetInfos")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|info| info.get("url").and_then(Value::as_str))
                    .any(|url| url.starts_with("devtools://"));
                if devtools_open {
                    return Err(fail(
                        Reason::RecordingConflict,
                        "a DevTools window is open and this browser cannot report which page it inspects; close it before filling",
                    ));
                }
                Ok(())
            }
            Err(error) => Err(protocol(error, Reason::ConnectFailed)),
        }
    }

    async fn attach(&mut self, page: &PageTarget) -> Result<(), FillError> {
        let attached = self
            .send(
                None,
                "Target.attachToTarget",
                json!({ "targetId": page.target_id, "flatten": true }),
            )
            .await
            .map_err(|error| protocol(error, Reason::PageAbsent))?;
        let session_id = attached
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                fail(
                    Reason::PageAbsent,
                    "attaching to the page returned no session",
                )
            })?
            .to_owned();
        self.page_session = Some(session_id.clone());
        self.enable_session(&session_id).await?;
        self.refresh_children().await
    }

    async fn enable_session(&mut self, session_id: &str) -> Result<(), FillError> {
        self.send(
            Some(session_id),
            "Target.setAutoAttach",
            json!({ "autoAttach": true, "waitForDebuggerOnStart": false, "flatten": true }),
        )
        .await
        .map_err(|error| protocol(error, Reason::PageAbsent))?;
        self.send(
            Some(session_id),
            "Emulation.setFocusEmulationEnabled",
            json!({ "enabled": true }),
        )
        .await
        .map_err(|error| protocol(error, Reason::PageAbsent))?;
        Ok(())
    }

    /// Registers out-of-process frames reported since the last call and
    /// drops detached ones. Only `iframe` targets become children: workers
    /// and other auto-attached targets have no frame and no Emulation
    /// domain, and they are released with the connection. A child whose
    /// setup fails has gone away; the chain resolution reports it if it was
    /// needed.
    async fn refresh_children(&mut self) -> Result<(), FillError> {
        loop {
            let mut changed = false;
            let connection = self.connection_mut()?;
            for event in connection.take_events("Target.detachedFromTarget") {
                let gone = event.params.get("sessionId").and_then(Value::as_str);
                self.children
                    .retain(|child| Some(child.session_id.as_str()) != gone);
            }
            let attached = self
                .connection_mut()?
                .take_events("Target.attachedToTarget");
            for event in attached {
                let session_id = event
                    .params
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let info = event.params.get("targetInfo");
                let target_type = info
                    .and_then(|info| info.get("type"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let target_id = info
                    .and_then(|info| info.get("targetId"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                if target_type != "iframe"
                    || session_id.is_empty()
                    || self
                        .children
                        .iter()
                        .any(|child| child.session_id == session_id)
                {
                    continue;
                }
                if self.enable_session(&session_id).await.is_err() {
                    continue;
                }
                self.children.push(Child {
                    target_id,
                    session_id,
                });
                changed = true;
            }
            if !changed {
                return Ok(());
            }
        }
    }

    async fn world(&mut self, frame: &FrameRef) -> Result<i64, FillError> {
        let key = (frame.session_id.clone(), frame.frame_id.clone());
        if let Some(context) = self.worlds.get(&key) {
            return Ok(*context);
        }
        let created = self
            .send(
                Some(&frame.session_id),
                "Page.createIsolatedWorld",
                json!({ "frameId": frame.frame_id, "worldName": WORLD_NAME, "grantUniveralAccess": false }),
            )
            .await
            .map_err(|error| protocol(error, Reason::TargetChanged))?;
        let context = created
            .get("executionContextId")
            .and_then(Value::as_i64)
            .ok_or_else(|| fail(Reason::TargetChanged, "no execution context for the frame"))?;
        self.worlds.insert(key, context);
        Ok(context)
    }

    /// Runs one fixed script and returns its `RemoteObject`. An exception or
    /// a destroyed context is reported with `changed_reason`.
    async fn call(
        &mut self,
        session_id: &str,
        target: Target,
        function: &str,
        arguments: Vec<Value>,
        by_value: bool,
        changed_reason: Reason,
    ) -> Result<Value, FillError> {
        let mut params = json!({
            "functionDeclaration": function,
            "arguments": arguments,
            "returnByValue": by_value,
            "awaitPromise": false,
        });
        match target {
            Target::Context(id) => params["executionContextId"] = json!(id),
            Target::Object(id) => params["objectId"] = json!(id),
        }
        let result = self
            .send(Some(session_id), "Runtime.callFunctionOn", params)
            .await
            .map_err(|error| protocol(error, changed_reason))?;
        if result.get("exceptionDetails").is_some() {
            return Err(fail(
                changed_reason,
                "a page-side check could not run; the selector may be invalid or the page changed",
            ));
        }
        Ok(result.get("result").cloned().unwrap_or(Value::Null))
    }

    async fn main_frame(&mut self) -> Result<FrameRef, FillError> {
        let session_id = self.page_session.clone().unwrap_or_default();
        let tree = self
            .send(Some(&session_id), "Page.getFrameTree", json!({}))
            .await
            .map_err(|error| protocol(error, Reason::TargetChanged))?;
        let frame_id = tree
            .get("frameTree")
            .and_then(|tree| tree.get("frame"))
            .and_then(|frame| frame.get("id"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        Ok(FrameRef {
            session_id,
            frame_id,
        })
    }

    /// Queries `selector` in `frame`: the match count and, for exactly one
    /// match, its object id.
    async fn query(
        &mut self,
        frame: &FrameRef,
        selector: &str,
        absent: Reason,
    ) -> Result<(usize, Option<String>), FillError> {
        let context = self.world(frame).await?;
        let array = self
            .call(
                &frame.session_id,
                Target::Context(context),
                scripts::QUERY_ALL,
                vec![json!({ "value": selector })],
                false,
                absent,
            )
            .await?;
        let array_id = array
            .get("objectId")
            .and_then(Value::as_str)
            .ok_or_else(|| fail(absent, "the selector query returned nothing"))?
            .to_owned();
        let count = self
            .call(
                &frame.session_id,
                Target::Object(array_id.clone()),
                scripts::COUNT,
                vec![],
                true,
                Reason::TargetChanged,
            )
            .await?
            .get("value")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;
        if count != 1 {
            return Ok((count, None));
        }
        let element = self
            .call(
                &frame.session_id,
                Target::Object(array_id),
                scripts::FIRST,
                vec![],
                false,
                Reason::TargetChanged,
            )
            .await?;
        Ok((
            1,
            element
                .get("objectId")
                .and_then(Value::as_str)
                .map(str::to_owned),
        ))
    }

    async fn resolve_chain(&mut self) -> Result<Vec<FrameRef>, FillError> {
        self.refresh_children().await?;
        let mut chain = vec![self.main_frame().await?];
        let selectors = self.target.frame_selectors.clone();
        for selector in &selectors {
            let parent = chain.last().cloned().unwrap_or_else(|| FrameRef {
                session_id: String::new(),
                frame_id: String::new(),
            });
            let owner = match self.query(&parent, selector, Reason::FrameAbsent).await? {
                (0, _) => {
                    return Err(fail(
                        Reason::FrameAbsent,
                        format!("no element matches frame selector '{selector}'"),
                    ))
                }
                (1, Some(owner)) => owner,
                (count, _) => {
                    return Err(fail(
                        Reason::FrameAmbiguous,
                        format!("{count} elements match frame selector '{selector}'; refine it"),
                    ))
                }
            };
            let is_owner = self
                .call(
                    &parent.session_id,
                    Target::Object(owner.clone()),
                    scripts::IS_FRAME_OWNER,
                    vec![],
                    true,
                    Reason::TargetChanged,
                )
                .await?
                .get("value")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if !is_owner {
                return Err(fail(
                    Reason::FrameInvalid,
                    format!("frame selector '{selector}' does not name an iframe"),
                ));
            }
            let described = self
                .send(
                    Some(&parent.session_id),
                    "DOM.describeNode",
                    json!({ "objectId": owner }),
                )
                .await
                .map_err(|error| protocol(error, Reason::TargetChanged))?;
            let frame_id = described
                .get("node")
                .and_then(|node| node.get("frameId"))
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    fail(
                        Reason::FrameInvalid,
                        format!("frame selector '{selector}' has no content frame"),
                    )
                })?
                .to_owned();
            // A frame in its own process has a child session; one that
            // attached during this resolution is picked up on a second look.
            let mut session_id = self.child_session(&frame_id);
            if session_id.is_none() {
                self.refresh_children().await?;
                session_id = self.child_session(&frame_id);
            }
            let session_id = session_id.unwrap_or_else(|| parent.session_id.clone());
            let frame = FrameRef {
                session_id,
                frame_id,
            };
            self.world(&frame).await?;
            chain.push(frame);
        }
        Ok(chain)
    }

    fn child_session(&self, frame_id: &str) -> Option<String> {
        self.children
            .iter()
            .find(|child| child.target_id == frame_id)
            .map(|child| child.session_id.clone())
    }

    async fn find_element(&mut self, chain: &[FrameRef]) -> Result<Element, FillError> {
        let frame = chain.last().cloned().unwrap_or_else(|| FrameRef {
            session_id: String::new(),
            frame_id: String::new(),
        });
        let selector = self.target.selector.clone();
        let object_id = match self.query(&frame, &selector, Reason::TargetAbsent).await? {
            (0, _) => {
                return Err(fail(
                    Reason::TargetAbsent,
                    format!("no element matches '{selector}'"),
                ))
            }
            (1, Some(id)) => id,
            (count, _) => {
                return Err(fail(
                    Reason::TargetAmbiguous,
                    format!("{count} elements match '{selector}'; refine the selector"),
                ))
            }
        };
        let state = self
            .call(
                &frame.session_id,
                Target::Object(object_id.clone()),
                scripts::CHECK_STATE,
                vec![],
                true,
                Reason::TargetChanged,
            )
            .await?;
        let state = state.get("value").cloned().unwrap_or(Value::Null);
        match state.get("state").and_then(Value::as_str) {
            Some("ok") => Ok(Element { frame, object_id }),
            Some("unfillable") => Err(fail(
                Reason::TargetUnfillable,
                "the element is not a fillable text control (input, textarea, or contenteditable)",
            )),
            Some("hidden") => Err(fail(Reason::TargetHidden, "the element is not visible")),
            Some("disabled") => Err(fail(Reason::TargetDisabled, "the element is disabled")),
            Some("readonly") => Err(fail(Reason::TargetReadonly, "the element is read-only")),
            _ => Err(fail(
                Reason::TargetChanged,
                "the element state could not be determined",
            )),
        }
    }

    /// The prepared element must still be connected, then the full selection
    /// is repeated and must yield the same node.
    async fn revalidate(
        &mut self,
        page: &PageTarget,
        chain: &[FrameRef],
        element: &Element,
    ) -> Result<(), FillError> {
        self.refresh_children().await?;
        let connected = self
            .call(
                &element.frame.session_id,
                Target::Object(element.object_id.clone()),
                scripts::IS_CONNECTED,
                vec![],
                true,
                Reason::TargetChanged,
            )
            .await?
            .get("value")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !connected {
            return Err(fail(
                Reason::TargetChanged,
                "the prepared element is no longer in the document",
            ));
        }
        let current_page = self.select_page().await?;
        if current_page.target_id != page.target_id {
            return Err(fail(
                Reason::TargetChanged,
                "a different page now matches the URL",
            ));
        }
        let current_chain = self.resolve_chain().await?;
        if current_chain != chain {
            return Err(fail(
                Reason::TargetChanged,
                "the frame chain changed after preparation",
            ));
        }
        let current = self.find_element(&current_chain).await?;
        let same = self
            .call(
                &element.frame.session_id,
                Target::Object(element.object_id.clone()),
                scripts::SAME_NODE,
                vec![json!({ "objectId": current.object_id })],
                true,
                Reason::TargetChanged,
            )
            .await?
            .get("value")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !same {
            return Err(fail(
                Reason::TargetChanged,
                "the target element was replaced after preparation",
            ));
        }
        Ok(())
    }
}

fn url_matches(mode: PageMatch, wanted: &str, actual: &str) -> bool {
    match mode {
        PageMatch::Exact => wanted == actual,
        PageMatch::OriginPath => matches!(
            (origin_path(wanted), origin_path(actual)),
            (Some(wanted), Some(actual)) if wanted == actual
        ),
    }
}

/// Scheme, host with port, and path of a URL; query and fragment removed.
fn origin_path(url: &str) -> Option<(String, String, String)> {
    let without_fragment = url.split('#').next()?;
    let without_query = without_fragment.split('?').next()?;
    let (scheme, rest) = without_query.split_once("://")?;
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    Some((
        scheme.to_ascii_lowercase(),
        authority.to_ascii_lowercase(),
        path.to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_path_ignores_query_and_fragment_but_not_origin_or_path() {
        assert!(url_matches(
            PageMatch::OriginPath,
            "http://127.0.0.1:8000/pages/login.html",
            "HTTP://127.0.0.1:8000/pages/login.html?state=1#x"
        ));
        assert!(!url_matches(
            PageMatch::OriginPath,
            "http://127.0.0.1:8000/pages/login.html",
            "http://localhost:8000/pages/login.html"
        ));
        assert!(!url_matches(
            PageMatch::OriginPath,
            "http://127.0.0.1:8000/pages/login.html",
            "http://127.0.0.1:8000/pages/other.html"
        ));
        assert!(!url_matches(
            PageMatch::Exact,
            "http://127.0.0.1:8000/pages/login.html",
            "http://127.0.0.1:8000/pages/login.html?state=1"
        ));
        assert!(
            !url_matches(PageMatch::OriginPath, "about:blank", "about:blank"),
            "URLs without an origin never match by origin and path"
        );
        assert!(!url_matches(
            PageMatch::OriginPath,
            "foo",
            "data:text/html,x"
        ));
    }
}
