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
            // The text goes in through the environment, never into the script, so it can't be taken for code. The
            // notifier is named by the AppUserModelID the installer's shortcut registers.
            let script = "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] > $null; \
                 $t = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02); \
                 $x = $t.GetElementsByTagName('text'); $x.Item(0).AppendChild($t.CreateTextNode($env:GITCRAFT_NOTIFY_TITLE)) > $null; $x.Item(1).AppendChild($t.CreateTextNode($env:GITCRAFT_NOTIFY_BODY)) > $null; \
                 [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('dev.gitcraft.desktop').Show([Windows.UI.Notifications.ToastNotification]::new($t))";
            Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", script]).env("GITCRAFT_NOTIFY_TITLE", &title).env("GITCRAFT_NOTIFY_BODY", &body).status()
        } else {
            Command::new("notify-send").args(["--app-name=GitCraft", &title, &body]).status()
        };
    });
}
