use objc2::{define_class, msg_send, rc::Retained, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSApplication, NSPasteboard};
use objc2_foundation::{NSArray, NSObject, NSString, NSURL};
use std::{cell::OnceCell, path::PathBuf};

use crate::{deliver_launch, LaunchIntent};

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = tauri::AppHandle]
    struct GitCraftFinderServices;

    impl GitCraftFinderServices {
        #[unsafe(method(openInGitCraft:userData:error:))]
        fn open_in_gitcraft(&self, pasteboard: &NSPasteboard, _user_data: Option<&NSString>, error: *mut *mut NSString) {
            let classes = NSArray::from_slice(&[NSURL::class()]);
            // SAFETY: AppKit delivers a valid pasteboard to this service callback on the main thread.
            let files = unsafe { pasteboard.readObjectsForClasses_options(&classes, None) };
            let paths: Vec<_> = files.iter().flat_map(|files| files.iter()).take(20)
                .filter_map(|object| object.downcast::<NSURL>().ok())
                .filter(|url| url.isFileURL())
                .filter_map(|url| url.path().map(|path| PathBuf::from(path.to_string())))
                .filter_map(|path| {
                    let path = if path.is_dir() { path } else { path.parent()?.to_owned() };
                    Some(LaunchIntent::Repository { path: path.to_string_lossy().into_owned() })
                })
                .collect();
            if paths.is_empty() {
                // SAFETY: AppKit passes either null or a writable out-pointer for the error string.
                if !error.is_null() { unsafe { *error = std::ptr::null_mut(); } }
                return;
            }
            deliver_launch(self.ivars(), paths);
        }
    }
);

pub fn register(app: tauri::AppHandle) {
    let Some(main) = MainThreadMarker::new() else { return; };
    thread_local! {
        static PROVIDER: OnceCell<Retained<GitCraftFinderServices>> = const { OnceCell::new() };
    }
    PROVIDER.with(|cell| {
        // SAFETY: called on the main thread (MainThreadMarker); `init` is NSObject's designated
        // initializer for this freshly allocated, ivar-initialized instance.
        let provider = cell.get_or_init(|| unsafe {
            msg_send![super(GitCraftFinderServices::alloc(main).set_ivars(app)), init]
        });
        // SAFETY: main-thread AppKit call; the provider is retained for the life of the process.
        unsafe { NSApplication::sharedApplication(main).setServicesProvider(Some(&**provider)); }
    });
}
