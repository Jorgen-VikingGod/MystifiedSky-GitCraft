//! Desktop notifications, through the system's own notifier.

use std::process::Command;

/// Show a system notification; nothing happens where none can be shown.
pub fn desktop_notification(title: &str, body: &str) {
    let (title, body) = (title.to_owned(), body.chars().take(300).collect::<String>());
    std::thread::spawn(move || {
        let _ = if cfg!(target_os = "macos") {
            let quote = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
            Command::new("osascript").args(["-e", &format!("display notification \"{}\" with title \"{}\"", quote(&body), quote(&title))]).status()
        } else if cfg!(windows) {
            let quote = |s: &str| s.replace('\'', "''");
            let script = format!(
                "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] > $null; \
                 $t = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02); \
                 $x = $t.GetElementsByTagName('text'); $x.Item(0).AppendChild($t.CreateTextNode('{}')) > $null; $x.Item(1).AppendChild($t.CreateTextNode('{}')) > $null; \
                 [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('GitCraft').Show([Windows.UI.Notifications.ToastNotification]::new($t))",
                quote(&title),
                quote(&body)
            );
            Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", &script]).status()
        } else {
            Command::new("notify-send").args(["--app-name=GitCraft", &title, &body]).status()
        };
    });
}
