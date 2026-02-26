use std::cell::RefCell;
use std::os::fd::{FromRawFd, OwnedFd};
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};

use glib::variant::ObjectPath;
use gtk4::gio;
use gtk4::gio::prelude::*;
use gtk4::glib;

/// Monotonically increasing counter shared across all portal sessions.
///
/// Using a per-session counter that resets to 1 every time means each new
/// recording subscribes to the same D-Bus request paths
/// (`.../gif_that_for_you_1`, `_2`, `_3`).  Old subscriptions are never
/// removed, so when a second recording fires on those paths the zombie
/// callbacks from the first session also wake up and make duplicate D-Bus
/// calls that the portal rejects.  A global counter guarantees every token
/// is unique for the lifetime of the process.
static GLOBAL_TOKEN: AtomicU32 = AtomicU32::new(1);

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
    /// Source types bitmask: 1=MONITOR, 2=WINDOW, 3=BOTH
    source_types: u32,
}

/// Begin the XDG ScreenCast portal flow.
///
/// The portal presents a system dialog asking the user which screen or window
/// to share.  On success, `on_ready` is called with the PipeWire fd and node
/// ID.  On failure or cancellation, `on_ready` receives an `Err`.
///
/// All D-Bus calls are asynchronous and integrated with the GLib main loop so
/// the UI stays responsive.
pub fn request_screencast<F>(source_types: u32, on_ready: F)
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
        source_types,
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

    call_portal_method(&state.borrow().connection, "CreateSession", &params, on_ready);
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
    let session_handle = match state.borrow().session_handle.clone() {
        Some(h) => h,
        None => {
            on_ready(Err("Portal: session_handle missing in SelectSources".into()));
            return;
        }
    };

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
    options.insert("types", state.borrow().source_types);
    // cursor_mode: 2 = EMBEDDED (draw cursor into the stream)
    options.insert("cursor_mode", 2u32);

    let obj_path = match ObjectPath::try_from(session_handle.as_str()) {
        Ok(p) => p,
        Err(e) => {
            on_ready(Err(format!("Portal: invalid session_handle in SelectSources: {e}")));
            return;
        }
    };
    let params =
        glib::Variant::tuple_from_iter([obj_path.to_variant(), options.end()]);

    call_portal_method(&state.borrow().connection, "SelectSources", &params, on_ready);
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
    let session_handle = match state.borrow().session_handle.clone() {
        Some(h) => h,
        None => {
            on_ready(Err("Portal: session_handle missing in Start".into()));
            return;
        }
    };

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

    let obj_path = match ObjectPath::try_from(session_handle.as_str()) {
        Ok(p) => p,
        Err(e) => {
            on_ready(Err(format!("Portal: invalid session_handle in Start: {e}")));
            return;
        }
    };
    let params = glib::Variant::tuple_from_iter([
        obj_path.to_variant(),
        String::new().to_variant(),
        options.end(),
    ]);

    call_portal_method(&state.borrow().connection, "Start", &params, on_ready);
}

// ---------------------------------------------------------------------------
// Portal flow: step 4 – OpenPipeWireRemote
// ---------------------------------------------------------------------------

fn step_open_pipewire_remote(
    state: Rc<RefCell<PortalState>>,
    node_id: u32,
    on_ready: Rc<dyn Fn(Result<PortalStream, String>)>,
) {
    let session_handle = match state.borrow().session_handle.clone() {
        Some(h) => h,
        None => {
            on_ready(Err("Portal: session_handle missing in OpenPipeWireRemote".into()));
            return;
        }
    };
    let connection = state.borrow().connection.clone();

    let options = glib::VariantDict::new(None);
    let obj_path = match ObjectPath::try_from(session_handle.as_str()) {
        Ok(p) => p,
        Err(e) => {
            on_ready(Err(format!("Portal: invalid session_handle in OpenPipeWireRemote: {e}")));
            return;
        }
    };
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
                // The result is a tuple (h,) where h is a Unix fd list index.
                // GVariant type 'h' has the same encoding as 'i' but is a
                // distinct type, so get::<i32>() returns None.  Use the GLib
                // C API directly to extract the handle value.
                let idx = unsafe {
                    glib::ffi::g_variant_get_handle(result.child_value(0).as_ptr())
                };
                if idx < 0 {
                    on_ready(Err("Portal: invalid fd index in OpenPipeWireRemote".into()));
                    return;
                }
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

fn next_token(_state: &Rc<RefCell<PortalState>>) -> String {
    let n = GLOBAL_TOKEN.fetch_add(1, Ordering::Relaxed);
    format!("gif_that_for_you_{n}")
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
        gio::DBusSignalFlags::NONE,
        move |_conn, _sender, _path, _iface, _signal, params| {
            // params is (uint32 response, a{sv} results)
            if params.n_children() < 2 {
                eprintln!("Portal: received Response signal with fewer than 2 arguments");
                return;
            }
            let response = params.child_value(0).get::<u32>().unwrap_or(2);
            let results = params.child_value(1);
            callback(response, results);
        },
    );
}

fn call_portal_method(
    connection: &gio::DBusConnection,
    method: &str,
    params: &glib::Variant,
    on_ready: Rc<dyn Fn(Result<PortalStream, String>)>,
) {
    let on_ready2 = on_ready.clone();
    let method_owned = method.to_string();
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
        move |result| {
            if let Err(e) = result {
                on_ready2(Err(format!("Portal: {method_owned} method call failed: {e}")));
            }
        },
    );
}

fn portal_error(step: &str, response: u32) -> String {
    match response {
        1 => format!("Portal: user cancelled at {step} (1)"),
        2 => format!("Portal: {step} failed: Other error (2)"),
        3 => format!("Portal: {step} failed: Not found (3)"),
        _ => format!("Portal: {step} failed (response code {response})"),
    }
}

/// Look up a string value in a portal `a{sv}` response dict.
fn variant_dict_lookup_str(dict_variant: &glib::Variant, key: &str) -> Option<String> {
    let dict_variant = if dict_variant.type_() == glib::VariantTy::VARIANT {
        dict_variant.child_value(0)
    } else {
        dict_variant.clone()
    };
    let dict = glib::VariantDict::new(Some(&dict_variant));
    dict.lookup::<String>(key).ok()?
}

/// Parse the `streams` array from the Start response to extract the first
/// PipeWire node ID.
///
/// The streams value is `a(ua{sv})` – an array of (node_id, properties).
fn parse_streams(results: &glib::Variant) -> Option<u32> {
    // Some portals wrap the results dictionary in an extra Variant.
    let results = if results.type_() == glib::VariantTy::VARIANT {
        results.child_value(0)
    } else {
        results.clone()
    };

    // Manually scan the a{sv} dict for the "streams" key.  Using
    // VariantDict::lookup::<Variant> can silently fail because the value is
    // stored as type 'v' in the dict and the returned variant may still carry
    // the extra 'v' wrapper depending on the glib-rs version.
    for i in 0..results.n_children() {
        let entry = results.child_value(i);
        if entry.n_children() < 2 {
            continue;
        }
        let Some(key) = entry.child_value(0).get::<String>() else {
            continue;
        };
        if key != "streams" {
            continue;
        }

        // In a{sv} the value is always stored as type 'v'; unbox it.
        let boxed = entry.child_value(1);
        let streams = if boxed.type_() == glib::VariantTy::VARIANT {
            boxed.child_value(0)
        } else {
            boxed
        };

        // streams is a(ua{sv}) — an array of (node_id, properties) tuples.
        if streams.n_children() == 0 {
            eprintln!("Portal: 'streams' array is empty in Start response");
            return None;
        }
        let first = streams.child_value(0);
        // first is (u, a{sv}) where child 0 is the node_id.
        if first.n_children() < 1 {
            return None;
        }
        return first.child_value(0).get::<u32>();
    }

    // "streams" not found — collect keys for debugging.
    let keys: Vec<String> = (0..results.n_children())
        .filter_map(|i| {
            let e = results.child_value(i);
            if e.n_children() >= 1 {
                e.child_value(0).get::<String>()
            } else {
                None
            }
        })
        .collect();
    eprintln!("Portal: 'streams' missing in Start response. Keys found: {:?}", keys);
    None
}

/// Extract a file descriptor from a `GUnixFDList` and wrap it in `OwnedFd`.
fn fd_list_steal(fd_list: gio::UnixFDList, idx: i32) -> Option<OwnedFd> {
    let raw_fd = fd_list.get(idx).ok()?;
    // `g_unix_fd_list_get` returns a dup'd fd, so we own it.
    Some(unsafe { OwnedFd::from_raw_fd(raw_fd) })
}
