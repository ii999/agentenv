//! The filling operation: connect, select, prepare, revalidate, insert, detach.

use std::collections::HashMap;
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::cdp::{CdpError, Connection};
use crate::outcome::{Failure, Outcome, Reason, Versions};
use crate::scripts;
use crate::{Args, PageMatch};

const WORLD_NAME: &str = "agentenv-fill-lab";
const CLEANUP_GRACE: Duration = Duration::from_secs(2);
const SUPPORTED_PROTOCOL: &str = "1.3";

pub struct Deadline {
    started: Instant,
    budget: Duration,
}

impl Deadline {
    pub fn start(budget: Duration) -> Self {
        Self {
            started: Instant::now(),
            budget,
        }
    }

    pub fn remaining(&self) -> Duration {
        self.budget.saturating_sub(self.started.elapsed())
    }

    pub fn expired(&self) -> bool {
        self.remaining().is_zero()
    }
}

#[derive(Clone, Debug)]
struct Child {
    target_id: String,
    session_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PageTarget {
    target_id: String,
    url: String,
    context_index: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FrameRef {
    session_id: String,
    frame_id: String,
    url: String,
    oopif: bool,
}

#[derive(Clone, Debug)]
struct Element {
    frame: FrameRef,
    object_id: String,
    tag: String,
    input_type: Option<String>,
}

struct Client<'a> {
    connection: Connection,
    deadline: &'a Deadline,
    versions: Versions,
    page_session: Option<String>,
    children: Vec<Child>,
    worlds: HashMap<(String, String), i64>,
}

pub fn run(args: &Args, value: &str, deadline: &Deadline) -> Result<Outcome, Failure> {
    let library = format!("cdp-fill-native {}", env!("CARGO_PKG_VERSION"));
    let (connection, version_document) = Connection::open(&args.endpoint, deadline.remaining())
        .map_err(|error| {
            let mut failure = protocol_failure(error, "connect", Reason::ConnectFailed);
            failure.versions.library = library.clone();
            failure
        })?;
    let mut client = Client {
        connection,
        deadline,
        versions: Versions {
            browser: version_document
                .get("Browser")
                .and_then(Value::as_str)
                .map(str::to_owned),
            protocol: None,
            library,
        },
        page_session: None,
        children: Vec::new(),
        worlds: HashMap::new(),
    };
    let result = client.operate(args, value);
    client.detach();
    result
}

fn protocol_failure(error: CdpError, phase: &'static str, reason: Reason) -> Failure {
    if error.timed_out {
        Failure::new(Reason::Timeout, phase, error.message)
    } else {
        Failure::new(reason, phase, error.message)
    }
}

impl Client<'_> {
    fn send(
        &mut self,
        session_id: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<Value, CdpError> {
        let remaining = self.deadline.remaining();
        self.connection.send(session_id, method, params, remaining)
    }

    fn fail(&self, reason: Reason, phase: &'static str, message: impl Into<String>) -> Failure {
        Failure::new(reason, phase, message).with_versions(&self.versions)
    }

    fn fail_protocol(&self, error: CdpError, phase: &'static str, reason: Reason) -> Failure {
        protocol_failure(error, phase, reason).with_versions(&self.versions)
    }

    fn operate(&mut self, args: &Args, value: &str) -> Result<Outcome, Failure> {
        self.check_versions()?;
        let page = self.select_page(args, "select-page")?;
        self.attach(&page)?;
        let chain = self.resolve_chain(args, "frame")?;
        let element = self.find_element(args, &chain, "element")?;

        if args.delay_before_insert_ms > 0 {
            let delay =
                Duration::from_millis(args.delay_before_insert_ms).min(self.deadline.remaining());
            sleep(delay);
        }
        if self.deadline.expired() {
            return Err(self.fail(
                Reason::Timeout,
                "delay",
                "the operation deadline expired before insertion; nothing was changed",
            ));
        }

        self.revalidate(args, &page, &chain, &element)?;
        self.insert(&element, value)?;

        let frame_chain: Vec<Value> = chain
            .iter()
            .skip(1)
            .map(|frame| json!({ "url": frame.url, "oopif": frame.oopif }))
            .collect();
        Ok(Outcome::filled(
            json!({
                "page": { "url": page.url, "contextIndex": page.context_index, "targetId": page.target_id },
                "frameChain": frame_chain,
                "elementTag": element.tag,
                "inputType": element.input_type,
            }),
            self.versions.clone(),
        ))
    }

    fn check_versions(&mut self) -> Result<(), Failure> {
        let version = self
            .send(None, "Browser.getVersion", json!({}))
            .map_err(|error| self.fail_protocol(error, "version", Reason::ConnectFailed))?;
        let product = version
            .get("product")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let protocol = version
            .get("protocolVersion")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if let Some(product) = product {
            self.versions.browser = Some(match &self.versions.browser {
                Some(browser) if browser != &product => format!("{product} ({browser})"),
                _ => product,
            });
        }
        self.versions.protocol = protocol.clone();
        if protocol.as_deref() != Some(SUPPORTED_PROTOCOL) {
            return Err(self.fail(
                Reason::VersionUnsupported,
                "version",
                format!(
                    "the browser speaks protocol version {}; {SUPPORTED_PROTOCOL} is required",
                    protocol.unwrap_or_else(|| "unknown".to_owned())
                ),
            ));
        }
        Ok(())
    }

    fn select_page(&mut self, args: &Args, phase: &'static str) -> Result<PageTarget, Failure> {
        let contexts = self
            .send(None, "Target.getBrowserContexts", json!({}))
            .map_err(|error| self.fail_protocol(error, phase, Reason::ConnectFailed))?;
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
        if let Some(index) = args.context_index {
            if index > context_ids.len() {
                return Err(self.fail(
                    Reason::ContextAbsent,
                    phase,
                    format!(
                        "context index {index} does not exist; {} contexts are open",
                        context_ids.len() + 1
                    ),
                ));
            }
        }
        let targets = self
            .send(None, "Target.getTargets", json!({}))
            .map_err(|error| self.fail_protocol(error, phase, Reason::ConnectFailed))?;
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
            if let Some(wanted) = args.context_index {
                if wanted != context_index {
                    continue;
                }
            }
            if !url_matches(args.page_match, &args.page_url, url) {
                continue;
            }
            matches.push(PageTarget {
                target_id: info
                    .get("targetId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                url: url.to_owned(),
                context_index,
            });
        }
        match matches.len() {
            0 => Err(self.fail(
                Reason::PageAbsent,
                phase,
                "no open page matches the requested URL",
            )),
            1 => Ok(matches.remove(0)),
            count => Err(self.fail(
                Reason::PageAmbiguous,
                phase,
                format!("{count} open pages match the requested URL; use --context-index or close duplicates"),
            )),
        }
    }

    fn attach(&mut self, page: &PageTarget) -> Result<(), Failure> {
        let attached = self
            .send(
                None,
                "Target.attachToTarget",
                json!({ "targetId": page.target_id, "flatten": true }),
            )
            .map_err(|error| self.fail_protocol(error, "attach", Reason::PageAbsent))?;
        let session_id = attached
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or_else(|| self.fail(Reason::PageAbsent, "attach", "attach returned no session"))?
            .to_owned();
        self.page_session = Some(session_id.clone());
        self.enable_session(&session_id, "attach")?;
        self.refresh_children("attach")
    }

    /// Enables auto-attach for child frame targets and focus emulation, as the
    /// Playwright reference does for every session it owns.
    fn enable_session(&mut self, session_id: &str, phase: &'static str) -> Result<(), Failure> {
        self.send(
            Some(session_id),
            "Target.setAutoAttach",
            json!({ "autoAttach": true, "waitForDebuggerOnStart": false, "flatten": true }),
        )
        .map_err(|error| self.fail_protocol(error, phase, Reason::PageAbsent))?;
        self.send(
            Some(session_id),
            "Emulation.setFocusEmulationEnabled",
            json!({ "enabled": true }),
        )
        .map_err(|error| self.fail_protocol(error, phase, Reason::PageAbsent))?;
        Ok(())
    }

    /// Registers child sessions reported since the last call and drops
    /// detached ones. Newly attached children get auto-attach themselves so
    /// nested out-of-process frames are reachable.
    fn refresh_children(&mut self, phase: &'static str) -> Result<(), Failure> {
        loop {
            let mut changed = false;
            for event in self.connection.take_events("Target.detachedFromTarget") {
                let gone = event.params.get("sessionId").and_then(Value::as_str);
                self.children
                    .retain(|child| Some(child.session_id.as_str()) != gone);
            }
            for event in self.connection.take_events("Target.attachedToTarget") {
                let session_id = event
                    .params
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let info = event
                    .params
                    .get("targetInfo")
                    .cloned()
                    .unwrap_or(Value::Null);
                let target_id = info
                    .get("targetId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                // Only frames become children; workers and other targets
                // have no frame and no Emulation domain. A child whose
                // setup fails has already gone away.
                if info.get("type").and_then(Value::as_str) != Some("iframe")
                    || session_id.is_empty()
                    || self.children.iter().any(|c| c.session_id == session_id)
                {
                    continue;
                }
                if self.enable_session(&session_id, phase).is_err() {
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

    fn world(&mut self, frame: &FrameRef, phase: &'static str) -> Result<i64, Failure> {
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
            .map_err(|error| self.fail_protocol(error, phase, Reason::TargetChanged))?;
        let context = created
            .get("executionContextId")
            .and_then(Value::as_i64)
            .ok_or_else(|| {
                self.fail(
                    Reason::TargetChanged,
                    phase,
                    "no execution context for the frame",
                )
            })?;
        self.worlds.insert(key, context);
        Ok(context)
    }

    /// Runs one fixed script. `target` is either an execution context (for
    /// functions without `this`) or an object id (`this`). Returns the raw
    /// `RemoteObject`; a thrown exception or destroyed context is reported
    /// with `changed_reason`.
    #[allow(clippy::too_many_arguments)] // prototype; a production client would carry a call context
    fn call(
        &mut self,
        session_id: &str,
        target: Target,
        function: &str,
        arguments: Vec<Value>,
        by_value: bool,
        phase: &'static str,
        changed_reason: Reason,
    ) -> Result<Value, Failure> {
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
            .map_err(|error| self.fail_protocol(error, phase, changed_reason))?;
        if let Some(exception) = result.get("exceptionDetails") {
            let text = exception
                .get("exception")
                .and_then(|value| value.get("description"))
                .and_then(Value::as_str)
                .unwrap_or("script exception");
            let summary: String = text
                .lines()
                .next()
                .unwrap_or_default()
                .chars()
                .take(160)
                .collect();
            return Err(self.fail(
                changed_reason,
                phase,
                format!("page script failed: {summary}"),
            ));
        }
        Ok(result.get("result").cloned().unwrap_or(Value::Null))
    }

    fn main_frame(&mut self, phase: &'static str) -> Result<FrameRef, Failure> {
        let session_id = self.page_session.clone().unwrap_or_default();
        let tree = self
            .send(Some(&session_id), "Page.getFrameTree", json!({}))
            .map_err(|error| self.fail_protocol(error, phase, Reason::TargetChanged))?;
        let frame = tree
            .get("frameTree")
            .and_then(|t| t.get("frame"))
            .cloned()
            .unwrap_or(Value::Null);
        Ok(FrameRef {
            session_id,
            frame_id: frame
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            url: frame
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            oopif: false,
        })
    }

    /// Queries `selector` in `frame`, returning the match count and, for a
    /// single match, its object id.
    fn query(
        &mut self,
        frame: &FrameRef,
        selector: &str,
        phase: &'static str,
        absent: Reason,
    ) -> Result<(usize, Option<String>), Failure> {
        let context = self.world(frame, phase)?;
        let array = self.call(
            &frame.session_id,
            Target::Context(context),
            scripts::QUERY_ALL,
            vec![json!({ "value": selector })],
            false,
            phase,
            absent,
        )?;
        let array_id = array
            .get("objectId")
            .and_then(Value::as_str)
            .ok_or_else(|| self.fail(absent, phase, "the selector query returned no array"))?
            .to_owned();
        let count = self
            .call(
                &frame.session_id,
                Target::Object(array_id.clone()),
                scripts::COUNT,
                vec![],
                true,
                phase,
                Reason::TargetChanged,
            )?
            .get("value")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;
        if count != 1 {
            return Ok((count, None));
        }
        let element = self.call(
            &frame.session_id,
            Target::Object(array_id),
            scripts::FIRST,
            vec![],
            false,
            phase,
            Reason::TargetChanged,
        )?;
        Ok((
            1,
            element
                .get("objectId")
                .and_then(Value::as_str)
                .map(str::to_owned),
        ))
    }

    fn resolve_chain(
        &mut self,
        args: &Args,
        phase: &'static str,
    ) -> Result<Vec<FrameRef>, Failure> {
        let mut chain = vec![self.main_frame(phase)?];
        for selector in &args.frame_selectors {
            let parent = chain.last().cloned().unwrap_or_else(unreachable_frame);
            let (count, owner) = self.query(&parent, selector, phase, Reason::FrameAbsent)?;
            let owner = match (count, owner) {
                (0, _) => {
                    return Err(self.fail(
                        Reason::FrameAbsent,
                        phase,
                        format!("no element matches frame selector '{selector}'"),
                    ))
                }
                (1, Some(owner)) => owner,
                (count, _) => {
                    return Err(self.fail(
                        Reason::FrameAmbiguous,
                        phase,
                        format!("{count} elements match frame selector '{selector}'"),
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
                    phase,
                    Reason::TargetChanged,
                )?
                .get("value")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if !is_owner {
                return Err(self.fail(
                    Reason::FrameInvalid,
                    phase,
                    format!("frame selector '{selector}' does not name an iframe"),
                ));
            }
            let described = self
                .send(
                    Some(&parent.session_id),
                    "DOM.describeNode",
                    json!({ "objectId": owner }),
                )
                .map_err(|error| self.fail_protocol(error, phase, Reason::TargetChanged))?;
            let frame_id = described
                .get("node")
                .and_then(|node| node.get("frameId"))
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    self.fail(
                        Reason::FrameInvalid,
                        phase,
                        format!("frame selector '{selector}' has no content frame"),
                    )
                })?
                .to_owned();
            let child = self
                .children
                .iter()
                .find(|child| child.target_id == frame_id)
                .cloned();
            let mut frame = match child {
                Some(child) => FrameRef {
                    session_id: child.session_id,
                    frame_id,
                    url: String::new(),
                    oopif: true,
                },
                None => FrameRef {
                    session_id: parent.session_id.clone(),
                    frame_id,
                    url: String::new(),
                    oopif: false,
                },
            };
            let context = self.world(&frame, phase)?;
            frame.url = self
                .call(
                    &frame.session_id,
                    Target::Context(context),
                    "function() { return location.href; }",
                    vec![],
                    true,
                    phase,
                    Reason::TargetChanged,
                )?
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            chain.push(frame);
        }
        Ok(chain)
    }

    fn find_element(
        &mut self,
        args: &Args,
        chain: &[FrameRef],
        phase: &'static str,
    ) -> Result<Element, Failure> {
        let frame = chain.last().cloned().unwrap_or_else(unreachable_frame);
        let (count, object_id) = self.query(&frame, &args.selector, phase, Reason::TargetAbsent)?;
        let object_id = match (count, object_id) {
            (0, _) => {
                return Err(self.fail(
                    Reason::TargetAbsent,
                    phase,
                    format!("no element matches '{}'", args.selector),
                ))
            }
            (1, Some(id)) => id,
            (count, _) => {
                return Err(self.fail(
                    Reason::TargetAmbiguous,
                    phase,
                    format!(
                        "{count} elements match '{}'; refine the selector",
                        args.selector
                    ),
                ))
            }
        };
        let state = self.call(
            &frame.session_id,
            Target::Object(object_id.clone()),
            scripts::CHECK_STATE,
            vec![],
            true,
            phase,
            Reason::TargetChanged,
        )?;
        let state = state.get("value").cloned().unwrap_or(Value::Null);
        let tag = state
            .get("tag")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let input_type = state
            .get("inputType")
            .and_then(Value::as_str)
            .map(str::to_owned);
        match state.get("state").and_then(Value::as_str) {
            Some("ok") => Ok(Element {
                frame,
                object_id,
                tag,
                input_type,
            }),
            Some("unfillable") => Err(self.fail(
                Reason::TargetUnfillable,
                phase,
                "the element is not a fillable text control",
            )),
            Some("hidden") => {
                Err(self.fail(Reason::TargetHidden, phase, "the element is not visible"))
            }
            Some("disabled") => {
                Err(self.fail(Reason::TargetDisabled, phase, "the element is disabled"))
            }
            Some("readonly") => {
                Err(self.fail(Reason::TargetReadonly, phase, "the element is read-only"))
            }
            _ => Err(self.fail(
                Reason::TargetChanged,
                phase,
                "the element state could not be determined",
            )),
        }
    }

    /// Same order as the reference client: the prepared element must still be
    /// connected (a navigation or detached frame is `target-changed`), then
    /// the full selection is repeated and must yield the same node.
    fn revalidate(
        &mut self,
        args: &Args,
        page: &PageTarget,
        chain: &[FrameRef],
        element: &Element,
    ) -> Result<(), Failure> {
        let phase = "revalidate";
        self.refresh_children(phase)?;
        let connected = self
            .call(
                &element.frame.session_id,
                Target::Object(element.object_id.clone()),
                "function() { return this.isConnected; }",
                vec![],
                true,
                phase,
                Reason::TargetChanged,
            )?
            .get("value")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !connected {
            return Err(self.fail(
                Reason::TargetChanged,
                phase,
                "the prepared element is no longer in the document",
            ));
        }
        let current_page = self.select_page(args, phase)?;
        if current_page.target_id != page.target_id {
            return Err(self.fail(
                Reason::TargetChanged,
                phase,
                "a different page now matches the URL",
            ));
        }
        let current_chain = self.resolve_chain(args, phase)?;
        let same_chain = current_chain.len() == chain.len()
            && current_chain
                .iter()
                .zip(chain)
                .all(|(a, b)| a.session_id == b.session_id && a.frame_id == b.frame_id);
        if !same_chain {
            return Err(self.fail(
                Reason::TargetChanged,
                phase,
                "the frame chain changed after preparation",
            ));
        }
        let current = self.find_element(args, &current_chain, phase)?;
        let same = self
            .call(
                &element.frame.session_id,
                Target::Object(element.object_id.clone()),
                scripts::SAME_NODE,
                vec![json!({ "objectId": current.object_id })],
                true,
                phase,
                Reason::TargetChanged,
            )?
            .get("value")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !same {
            return Err(self.fail(
                Reason::TargetChanged,
                phase,
                "the target element was replaced after preparation",
            ));
        }
        Ok(())
    }

    fn insert(&mut self, element: &Element, value: &str) -> Result<(), Failure> {
        let phase = "insert";
        if self.deadline.expired() {
            return Err(self.fail(
                Reason::Timeout,
                phase,
                "the operation deadline expired before insertion; nothing was changed",
            ));
        }
        self.call(
            &element.frame.session_id,
            Target::Object(element.object_id.clone()),
            scripts::SELECT_AND_FOCUS,
            vec![],
            true,
            phase,
            Reason::TargetChanged,
        )?;
        let page_session = self.page_session.clone().unwrap_or_default();
        self.send(
            Some(&page_session),
            "Input.insertText",
            json!({ "text": value }),
        )
        .map_err(|error| {
            if error.timed_out {
                self.fail(
                    Reason::Timeout,
                    phase,
                    "the deadline expired during insertion; the field may have changed",
                )
            } else {
                self.fail(
                    Reason::TargetChanged,
                    phase,
                    format!(
                        "insertion failed; the field may have changed: {}",
                        error.message
                    ),
                )
            }
        })?;
        Ok(())
    }

    /// Detaches this client's sessions and closes the socket. Never closes a
    /// target. Uses its own short grace period so cleanup happens even after
    /// the operation deadline expired.
    fn detach(&mut self) {
        let grace = Deadline::start(CLEANUP_GRACE);
        let children: Vec<String> = self.children.iter().map(|c| c.session_id.clone()).collect();
        for session_id in children.into_iter().chain(self.page_session.take()) {
            let _ = self.connection.send(
                None,
                "Target.detachFromTarget",
                json!({ "sessionId": session_id }),
                grace.remaining(),
            );
        }
    }
}

enum Target {
    Context(i64),
    Object(String),
}

fn unreachable_frame() -> FrameRef {
    FrameRef {
        session_id: String::new(),
        frame_id: String::new(),
        url: String::new(),
        oopif: false,
    }
}

fn url_matches(mode: PageMatch, wanted: &str, actual: &str) -> bool {
    match mode {
        PageMatch::Exact => wanted == actual,
        PageMatch::OriginPath => origin_path(wanted) == origin_path(actual),
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
    fn origin_path_ignores_query_and_fragment() {
        assert_eq!(
            origin_path("http://127.0.0.1:8000/pages/login.html?state=1#x"),
            origin_path("HTTP://127.0.0.1:8000/pages/login.html")
        );
        assert_ne!(
            origin_path("http://localhost:8000/pages/login.html"),
            origin_path("http://127.0.0.1:8000/pages/login.html")
        );
        assert_ne!(
            origin_path("http://127.0.0.1:8000/pages/login.html"),
            origin_path("http://127.0.0.1:8000/pages/other.html")
        );
    }
}
