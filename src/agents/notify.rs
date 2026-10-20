//! Desktop notifications for sessions that need the user while tether is in the background.

/// Shows a native toast without blocking the UI (WinRT / D-Bus can take a while).
pub fn desktop(title: String, body: String) {
    let _ = std::thread::Builder::new().name("tether-toast".into()).spawn(move || {
        let _ = notify_rust::Notification::new().appname("tether").summary(&title).body(&body).show();
    });
}
