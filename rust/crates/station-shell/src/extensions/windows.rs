use super::{Extension, Reply, Request, Update};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2BrowserExtension, ICoreWebView2BrowserExtensionList, ICoreWebView2Profile7,
    ICoreWebView2_13,
};
use webview2_com::{
    BrowserExtensionRemoveCompletedHandler, ProfileAddBrowserExtensionCompletedHandler,
    ProfileGetBrowserExtensionsCompletedHandler,
};
use windows_core::{Interface, HSTRING, PWSTR};
use wry::WebViewExtWindows;

pub(crate) fn request(view: &wry::WebView, request: Request, reply: Reply) {
    // SAFETY: the Wry view and its profile are used on the creating UI thread. The WebView2
    // methods retain the COM callback for the duration of each operation.
    let result = unsafe {
        view.controller()
            .CoreWebView2()
            .and_then(|webview| webview.cast::<ICoreWebView2_13>())
            .and_then(|webview| webview.Profile())
            .and_then(|profile| profile.cast::<ICoreWebView2Profile7>())
            .and_then(|profile| start(&profile, request, reply.clone()))
    };
    if let Err(error) = result {
        reply.send(Err(format!("WebView2 extension request failed ({error}).")));
    }
}

unsafe fn start(
    profile: &ICoreWebView2Profile7,
    request: Request,
    reply: Reply,
) -> windows_core::Result<()> {
    match request {
        Request::Install(folder) => {
            let callback = ProfileAddBrowserExtensionCompletedHandler::create(Box::new(
                move |status, extension| {
                    let result = status
                        .map_err(|error| format!("Extension installation failed ({error})."))
                        .and_then(|()| {
                            extension
                                .ok_or_else(|| "WebView2 did not return the extension.".to_owned())
                        })
                        .and_then(|extension| info(&extension).map(Update::Installed));
                    reply.send(result);
                    Ok(())
                },
            ));
            // SAFETY: the profile is on its creating apartment. The completed callback is owned
            // by COM until invocation, and the input path is alive for the call.
            unsafe { profile.AddBrowserExtension(&HSTRING::from(folder.as_path()), &callback) }
        }
        Request::List | Request::Remove(_) => {
            let callback = ProfileGetBrowserExtensionsCompletedHandler::create(Box::new(
                move |status, list| {
                    let result = status
                        .map_err(|error| format!("Could not list extensions ({error})."))
                        .and_then(|()| {
                            list.ok_or_else(|| {
                                "WebView2 did not return an extension list.".to_owned()
                            })
                        })
                        .and_then(|list| collect(&list));
                    match (request, result) {
                        (Request::Remove(id), Ok(entries)) => remove(id, entries, reply),
                        (_, Ok(entries)) => reply.send(Ok(Update::List(
                            entries.into_iter().map(|(entry, _)| entry).collect(),
                        ))),
                        (_, Err(error)) => reply.send(Err(error)),
                    }
                    Ok(())
                },
            ));
            // SAFETY: WebView2 invokes this callback asynchronously on the calling UI thread.
            unsafe { profile.GetBrowserExtensions(&callback) }
        }
    }
}

fn info(extension: &ICoreWebView2BrowserExtension) -> Result<Extension, String> {
    let mut id = PWSTR::null();
    let mut name = PWSTR::null();
    // SAFETY: WebView2 provides CoTaskMem strings. Each successful read transfers memory to
    // take_pwstr, which handles null pointers and releases the allocation.
    unsafe {
        extension.Id(&mut id).map_err(|error| error.to_string())?;
        let id = webview2_com::take_pwstr(id);
        extension
            .Name(&mut name)
            .map_err(|error| error.to_string())?;
        Ok(Extension {
            id,
            name: webview2_com::take_pwstr(name),
        })
    }
}

fn collect(
    list: &ICoreWebView2BrowserExtensionList,
) -> Result<Vec<(Extension, ICoreWebView2BrowserExtension)>, String> {
    let mut count = 0;
    // SAFETY: the callback delivered the list to the WebView2 UI thread, and each index is in
    // bounds for this snapshot.
    unsafe {
        list.Count(&mut count).map_err(|error| error.to_string())?;
        (0..count)
            .map(|index| {
                let extension = list
                    .GetValueAtIndex(index)
                    .map_err(|error| error.to_string())?;
                Ok((info(&extension)?, extension))
            })
            .collect()
    }
}

fn remove(id: String, entries: Vec<(Extension, ICoreWebView2BrowserExtension)>, reply: Reply) {
    let Some((_, extension)) = entries.into_iter().find(|(entry, _)| entry.id == id) else {
        reply.send(Err("The extension is gone. Refresh the list.".to_owned()));
        return;
    };
    let done = reply.clone();
    let callback = BrowserExtensionRemoveCompletedHandler::create(Box::new(move |status| {
        done.send(
            status
                .map(|()| Update::Removed(id))
                .map_err(|error| format!("Extension removal failed ({error}).")),
        );
        Ok(())
    }));
    // SAFETY: the extension was obtained from this profile on the UI thread. WebView2 retains
    // the completion handler until it can report the result.
    if let Err(error) = unsafe { extension.Remove(&callback) } {
        reply.send(Err(format!("Extension removal failed ({error}).")));
    }
}
