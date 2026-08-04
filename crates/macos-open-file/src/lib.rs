#![deny(unsafe_code)]

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod macos {
    use std::path::PathBuf;

    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2::{ClassType, DeclaredClass, declare_class, msg_send_id, mutability};
    use objc2_app_kit::{NSApplication, NSApplicationDelegate};
    use objc2_foundation::{MainThreadMarker, NSArray, NSObject, NSObjectProtocol, NSURL};

    struct OpenFileDelegateIvars {
        on_open: Box<dyn Fn(PathBuf)>,
    }

    declare_class!(
        struct OpenFileDelegate;

        unsafe impl ClassType for OpenFileDelegate {
            type Super = NSObject;
            type Mutability = mutability::MainThreadOnly;
            const NAME: &'static str = "TortoOpenFileDelegate";
        }

        impl DeclaredClass for OpenFileDelegate {
            type Ivars = OpenFileDelegateIvars;
        }

        unsafe impl NSObjectProtocol for OpenFileDelegate {}

        unsafe impl NSApplicationDelegate for OpenFileDelegate {
            #[method(application:openURLs:)]
            fn application_open_urls(&self, _application: &NSApplication, urls: &NSArray<NSURL>) {
                for url in urls {
                    if !unsafe { url.isFileURL() } {
                        continue;
                    }
                    let Some(path) = (unsafe { url.path() }) else {
                        continue;
                    };
                    (self.ivars().on_open)(path.to_string().into());
                }
            }
        }
    );

    impl OpenFileDelegate {
        fn new(on_open: Box<dyn Fn(PathBuf)>, mtm: MainThreadMarker) -> Retained<Self> {
            let this = mtm.alloc().set_ivars(OpenFileDelegateIvars { on_open });
            unsafe { msg_send_id![super(this), init] }
        }
    }

    /// Keeps the AppKit delegate alive for as long as file-open events are needed.
    pub struct OpenFileHandler {
        _delegate: Retained<OpenFileDelegate>,
    }

    /// Installs the application delegate used by Finder and Launch Services.
    pub fn install(on_open: impl Fn(PathBuf) + 'static) -> Result<OpenFileHandler, std::io::Error> {
        let mtm = MainThreadMarker::new().ok_or_else(|| {
            std::io::Error::other("macOS application must start on the main thread")
        })?;
        let delegate = OpenFileDelegate::new(Box::new(on_open), mtm);
        let application = NSApplication::sharedApplication(mtm);
        application.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        Ok(OpenFileHandler {
            _delegate: delegate,
        })
    }
}

#[cfg(target_os = "macos")]
pub use macos::{OpenFileHandler, install};
