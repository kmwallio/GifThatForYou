use std::cell::RefCell;
use std::os::fd::{FromRawFd, OwnedFd};
use std::rc::Rc;

use glib::variant::ObjectPath;
use gtk4::gio;
use gtk4::gio::prelude::*;
use gtk4::glib;

const PORTAL_BUS_NAME: &str = "org.freedesktop.portal.Desktop";
const PORTAL_OBJECT_PATH: &str = "/org/freedesktop/portal/desktop";
const SCREENCAST_IFACE: &str = "org.freedesktop.portal.ScreenCast";
const REQUEST_IFACE: &str = "org.freedesktop.portal.Request";

/// Result of a successful portal screencast negotiation.
pub struct PortalStream {
    /// PipeWire file descriptor for the screencast session.
    pub fd: OwnedFd,
    /// PipeWire node ID of the video stream.
    pub node_id: u32,
}

/// Internal state that accumulates across the portal callback chain.
struct PortalState {
    connection: gio::DBusConnection,
    session_handle: Option<String>,
    counter: u32,
}

/// Begin the XDG ScreenCast portal flow.
///
/// The portal presents a system dialog asking the user which screen or window
/// to share.  On success, `on_ready` is called with the PipeWire fd and node
/// ID.  On failure or cancellation, `on_ready` receives an `Err`.
///
/// All D-Bus calls are asynchronous and integrated with the GLib main loop so
/// the UI stays responsive.
pub fn request_screencast<F>(on_ready: F)
where
    F: Fn(Result<PortalStream, String>) + 'static,
{
    let connection = match gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) {
        Ok(c) => c,
        Err(e) => {
            on_ready(Err(format!("Failed to connect to session bus: {e}")));
            return;
        }
    };

    let state = Rc::new(RefCell::new(PortalState {
        connection: connection.clone(),
        session_handle: None,
        counter: 0,
    }));

    let on_ready = Rc::new(on_ready);

    // Step 1 → CreateSession
    step_create_session(state, on_ready);
}

// ---------------------------------------------------------------------------
// Portal flow: step 1 – CreateSession
// ---------------------------------------------------------------------------

fn step_create_session(
    state: Rc<RefCell<PortalState>>,
    on_ready: Rc<dyn Fn(Result<PortalStream, String>)>,
) {
    let token = next_token(&state);
    let session_token = next_token(&state);
    let request_path = make_request_path(&state.borrow().connection, &token);

    // Subscribe to the Response signal *before* making the call.
    let state2 = state.clone();
    let on_ready2 = on_ready.clone();
    subscribe_response(
        &state.borrow().connection,
        &request_path,
        move |response, results| {
            if response != 0 {
                on_ready2(Err(portal_error("CreateSession", response)));
                return;
            }
            if let Some(handle) = variant_dict_lookup_str(&results, "session_handle") {
                state2.borrow_mut().session_handle = Some(handle);
                step_select_sources(state2.clone(), on_ready2.clone());
            } else {
                on_ready2(Err(
                    "Portal: missing session_handle in CreateSession response".into(),
                ));
            }
        },
    );

    let options = glib::VariantDict::new(None);
    options.insert("handle_token", &token);
    options.insert("session_handle_token", &session_token);
    let params = glib::Variant::tuple_from_iter([options.end()]);

    call_portal_method(&state.borrow().connection, "CreateSession", &params);
}

// ---------------------------------------------------------------------------
// Portal flow: step 2 – SelectSources
// ---------------------------------------------------------------------------

fn step_select_sources(
    state: Rc<RefCell<PortalState>>,
    on_ready: Rc<dyn Fn(Result<PortalStream, String>)>,
) {
    let token = next_token(&state);
    let request_path = make_request_path(&state.borrow().connection, &token);
    let session_handle = state.borrow().session_handle.clone().unwrap();

    let state2 = state.clone();
    let on_ready2 = on_ready.clone();
    subscribe_response(
        &state.borrow().connection,
        &request_path,
        move |response, _results| {
            if response != 0 {
                on_ready2(Err(portal_error("SelectSources", response)));
                return;
            }
            step_start(state2.clone(), on_ready2.clone());
        },
    );

    let options = glib::VariantDict::new(None);
    options.insert("handle_token", &token);
    // types: 1 = MONITOR, 2 = WINDOW, 3 = BOTH
    options.insert("types", 3u32);
    // cursor_mode: 2 = EMBEDDED (draw cursor into the stream)
    options.insert("cursor_mode", 2u32);

    let obj_path = ObjectPath::try_from(session_handle.as_str()).unwrap();
    let params =
        glib::Variant::tuple_from_iter([obj_path.to_variant(), options.end()]);

    call_portal_method(&state.borrow().connection, "SelectSources", &params);
}

// ---------------------------------------------------------------------------
// Portal flow: step 3 – Start
// ---------------------------------------------------------------------------

fn step_start(
    state: Rc<RefCell<PortalState>>,
    on_ready: Rc<dyn Fn(Result<PortalStream, String>)>,
) {
    let token = next_token(&state);
    let request_path = make_request_path(&state.borrow().connection, &token);
    let session_handle = state.borrow().session_handle.clone().unwrap();

    let state2 = state.clone();
    let on_ready2 = on_ready.clone();
    subscribe_response(
        &state.borrow().connection,
        &request_path,
        move |response, results| {
            if response != 0 {
                on_ready2(Err(portal_error("Start", response)));
                return;
            }
            match parse_streams(&results) {
                Some(node_id) => {
                    step_open_pipewire_remote(state2.clone(), node_id, on_ready2.clone());
                }
                None => {
                    on_ready2(Err("Portal: no streams in Start response".into()));
                }
            }
        },
    );

    let options = glib::VariantDict::new(None);
    options.insert("handle_token", &token);

    let obj_path = ObjectPath::try_from(session_handle.as_str()).unwrap();
    let params = glib::Variant::tuple_from_iter([
        obj_path.to_variant(),
        String::new().to_variant(),
        options.end(),
    ]);

    call_portal_method(&state.borrow().connection, "Start", &params);
}

// ---------------------------------------------------------------------------
// Portal flow: step 4 – OpenPipeWireRemote
// ---------------------------------------------------------------------------

fn step_open_pipewire_remote(
    state: Rc<RefCell<PortalState>>,
    node_id: u32,
    on_ready: Rc<dyn Fn(Result<PortalStream, String>)>,
) {
    let session_handle = state.borrow().session_handle.clone().unwrap();
    let connection = state.borrow().connection.clone();

    let options = glib::VariantDict::new(None);
    let obj_path = ObjectPath::try_from(session_handle.as_str()).unwrap();
    let params =
        glib::Variant::tuple_from_iter([obj_path.to_variant(), options.end()]);

    // This method returns a file descriptor via GUnixFDList.
    match connection.call_with_unix_fd_list_sync(
        Some(PORTAL_BUS_NAME),
        PORTAL_OBJECT_PATH,
        SCREENCAST_IFACE,
        "OpenPipeWireRemote",
        Some(&params),
        None,
        gio::DBusCallFlags::NONE,
        -1,
        None::<&gio::UnixFDList>,
        gio::Cancellable::NONE,
    ) {
        Ok((result, fd_list)) => {
            if let Some(fd_list) = fd_list {
                // The result is a tuple (h,) where h is an index into the fd
                // list.
                let idx = result.child_get::<i32>(0);
                match fd_list_steal(fd_list, idx) {
                    Some(fd) => {
                        on_ready(Ok(PortalStream { fd, node_id }));
                    }
                    None => {
                        on_ready(Err(
                            "Portal: no file descriptor in OpenPipeWireRemote".into(),
                        ));
                    }
                }
            } else {
                on_ready(Err("Portal: no fd list in OpenPipeWireRemote".into()));
            }
        }
        Err(e) => {
            on_ready(Err(format!("Portal: OpenPipeWireRemote failed: {e}")));
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn next_token(state: &Rc<RefCell<PortalState>>) -> String {
    let mut s = state.borrow_mut();
    s.counter += 1;
    format!("gif_that_for_you_{}", s.counter)
}

fn make_request_path(connection: &gio::DBusConnection, token: &str) -> String {
    let sender = connection
        .unique_name()
        .map(|n| n.to_string())
        .unwrap_or_default()
        .replace('.', "_")
        .replace(':', "");
    format!("/org/freedesktop/portal/desktop/request/{sender}/{token}")
}

/// Subscribe to the one-shot `Response` signal on the given request path.
fn subscribe_response<F>(connection: &gio::DBusConnection, path: &str, callback: F)
where
    F: Fn(u32, glib::Variant) + 'static,
{
    let path_owned = path.to_string();

    // We cannot capture the subscription id inside its own callback at
    // construction time, so we skip unsubscription; the portal guarantees
    // each Request path receives at most one Response signal.
    let _id = connection.signal_subscribe(
        Some(PORTAL_BUS_NAME),
        Some(REQUEST_IFACE),
        Some("Response"),
        Some(&path_owned),
        None,
        gio::DBusSignalFlags::NO_MATCH_RULE,
        move |_conn, _sender, _path, _iface, _signal, params| {
            // params is (uint32 response, a{sv} results)
            let response = params.child_get::<u32>(0);
            let results = params.child_get::<glib::Variant>(1);
            callback(response, results);
        },
    );
}

fn call_portal_method(
    connection: &gio::DBusConnection,
    method: &str,
    params: &glib::Variant,
) {
    connection.call(
        Some(PORTAL_BUS_NAME),
        PORTAL_OBJECT_PATH,
        SCREENCAST_IFACE,
        method,
        Some(params),
        None,
        gio::DBusCallFlags::NONE,
        -1,
        gio::Cancellable::NONE,
        |_result| {
            // The actual response comes via the Request signal; we ignore the
            // method return value here (it's just the request object path).
        },
    );
}

fn portal_error(step: &str, response: u32) -> String {
    match response {
        1 => format!("Portal: user cancelled at {step}"),
        _ => format!("Portal: {step} failed (response code {response})"),
    }
}

/// Look up a string value in a portal `a{{sv}}` response dict.
fn variant_dict_lookup_str(dict_variant: &glib::Variant, key: &str) -> Option<String> {
    let dict = glib::VariantDict::new(Some(dict_variant));
    dict.lookup::<String>(key).ok()?
}

/// Parse the `streams` array from the Start response to extract the first
/// PipeWire node ID.
///
/// The streams value is `a(ua{{sv}})` – an array of (node_id, properties).
fn parse_streams(results: &glib::Variant) -> Option<u32> {
    let dict = glib::VariantDict::new(Some(results));
    let streams = dict.lookup::<glib::Variant>("streams").ok()??;
    // streams is a(ua{sv})
    let first = streams.child_value(0);
    // first is (ua{sv})
    let node_id = first.child_get::<u32>(0);
    Some(node_id)
}

/// Extract a file descriptor from a `GUnixFDList` and wrap it in `OwnedFd`.
fn fd_list_steal(fd_list: gio::UnixFDList, idx: i32) -> Option<OwnedFd> {
    let raw_fd = fd_list.get(idx).ok()?;
    // `g_unix_fd_list_get` returns a dup'd fd, so we own it.
    Some(unsafe { OwnedFd::from_raw_fd(raw_fd) })
}
