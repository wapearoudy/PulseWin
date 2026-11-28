//! Read the already-open Models & Usage page through Windows UI Automation.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::Path;
use std::time::{Duration, Instant};

use windows::core::{Interface, BOOL, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    CUIAutomation8, IUIAutomation, IUIAutomation2, IUIAutomationCondition, IUIAutomationElement,
    PropertyConditionFlags_IgnoreCase, TreeScope_Descendants, UIA_ControlTypePropertyId,
    UIA_IsOffscreenPropertyId, UIA_NamePropertyId, UIA_TextControlTypeId, UIA_WindowControlTypeId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GetClassNameW, GetWindowTextW, GetWindowThreadProcessId,
    IsIconic, IsWindowVisible,
};

use super::desktop_page::{is_models_page_heading, MODELS_PAGE_HEADINGS};

const TRANSACTION_TIMEOUT_MS: u32 = 1_000;
const CAPTURE_BUDGET: Duration = Duration::from_secs(2);
const MAX_SIBLINGS: usize = 192;
const MAX_TEXT_LENGTH: usize = 256;
const MAX_RENDERER_WINDOWS: usize = 8;

struct Apartment;

impl Drop for Apartment {
    fn drop(&mut self) {
        // Instances are created only after a successful CoInitializeEx call.
        unsafe { CoUninitialize() }
    }
}

/// Call on a blocking, windowless worker. Only plain text leaves the apartment.
pub(super) fn read_model_page() -> Option<Vec<String>> {
    let deadline = Instant::now() + CAPTURE_BUDGET;
    // SAFETY: HWNDs and COM objects are used on this thread, within their lifetimes.
    unsafe {
        let window = antigravity_window()?;
        CoInitializeEx(None, COINIT_MULTITHREADED).ok().ok()?;
        let _apartment = Apartment;
        let automation: IUIAutomation =
            CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER).ok()?;
        let timeouts: IUIAutomation2 = automation.cast().ok()?;
        timeouts.SetConnectionTimeout(TRANSACTION_TIMEOUT_MS).ok()?;
        timeouts
            .SetTransactionTimeout(TRANSACTION_TIMEOUT_MS)
            .ok()?;

        let root = automation.ElementFromHandle(window).ok()?;
        let heading = match models_heading(&automation, &root).ok()? {
            Some(heading) => heading,
            None => renderer_heading(&automation, window, deadline)?,
        };
        let names = visible_text_siblings(&automation, &heading, deadline)?;
        if !is_visible_window(window) || heading.CurrentIsOffscreen().ok()?.as_bool() {
            return None;
        }
        Some(names)
    }
}

unsafe fn antigravity_window() -> Option<HWND> {
    let mut found = None::<HWND>;
    // EnumWindows calls the callback synchronously, so the pointer stays valid.
    EnumWindows(Some(select_window), LPARAM(&mut found as *mut _ as isize)).ok()?;
    found
}

/// The same verified window scope is used for an explicit visual refresh.
pub(super) fn target_window() -> Option<HWND> {
    // SAFETY: the callback only queries window and process-image metadata.
    unsafe { antigravity_window() }
}

unsafe extern "system" fn select_window(window: HWND, state: LPARAM) -> BOOL {
    if !is_visible_window(window) {
        return BOOL(1);
    }
    let mut title = [0u16; 32];
    let length = GetWindowTextW(window, &mut title) as usize;
    if length != "Antigravity".len()
        || !String::from_utf16_lossy(&title[..length]).eq_ignore_ascii_case("Antigravity")
        || !is_antigravity_process(window)
    {
        return BOOL(1);
    }

    let found = &mut *(state.0 as *mut Option<HWND>);
    if found.is_some() {
        // Different visible instances might represent different accounts.
        *found = None;
        return BOOL(0);
    }
    *found = Some(window);
    BOOL(1)
}

unsafe fn is_visible_window(window: HWND) -> bool {
    IsWindowVisible(window).as_bool() && !IsIconic(window).as_bool()
}

unsafe fn is_antigravity_process(window: HWND) -> bool {
    let mut pid = 0u32;
    if GetWindowThreadProcessId(window, Some(&mut pid)) == 0 || pid == 0 {
        return false;
    }
    let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
        return false;
    };
    let mut path = [0u16; 2048];
    let mut length = path.len() as u32;
    let result = QueryFullProcessImageNameW(
        process,
        PROCESS_NAME_WIN32,
        PWSTR(path.as_mut_ptr()),
        &mut length,
    );
    let _ = CloseHandle(process);
    if result.is_err() || length as usize > path.len() {
        return false;
    }
    let path = OsString::from_wide(&path[..length as usize]);
    Path::new(&path)
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("Antigravity.exe"))
}

unsafe fn renderer_heading(
    automation: &IUIAutomation,
    window: HWND,
    deadline: Instant,
) -> Option<IUIAutomationElement> {
    // Chromium can expose document controls through its native renderer child.
    let mut renderers = Vec::<HWND>::new();
    let _ = EnumChildWindows(
        Some(window),
        Some(select_renderer),
        LPARAM(&mut renderers as *mut _ as isize),
    );
    if renderers.len() > MAX_RENDERER_WINDOWS {
        return None;
    }

    let mut heading = None;
    for renderer in renderers {
        if Instant::now() >= deadline {
            return None;
        }
        let element = automation.ElementFromHandle(renderer).ok()?;
        if let Some(candidate) = models_heading(automation, &element).ok()? {
            if heading.replace(candidate).is_some() {
                return None;
            }
        }
    }
    heading
}

unsafe extern "system" fn select_renderer(window: HWND, state: LPARAM) -> BOOL {
    const CHROME_RENDERER_CLASS: &str = "Chrome_RenderWidgetHostHWND";
    let mut class = [0u16; 64];
    let length = GetClassNameW(window, &mut class) as usize;
    if length != CHROME_RENDERER_CLASS.len()
        || !String::from_utf16_lossy(&class[..length]).eq_ignore_ascii_case(CHROME_RENDERER_CLASS)
        || !is_antigravity_process(window)
    {
        return BOOL(1);
    }
    // EnumChildWindows invokes this callback on the calling thread.
    let renderers = &mut *(state.0 as *mut Vec<HWND>);
    renderers.push(window);
    BOOL((renderers.len() <= MAX_RENDERER_WINDOWS).into())
}

unsafe fn models_heading(
    automation: &IUIAutomation,
    root: &IUIAutomationElement,
) -> Result<Option<IUIAutomationElement>, ()> {
    let suffixes = ["", " Refresh quota and credits data", " 刷新额度与积分"];
    let name_matches: Vec<Option<IUIAutomationCondition>> = MODELS_PAGE_HEADINGS
        .iter()
        .flat_map(|heading| suffixes.map(|suffix| format!("{heading}{suffix}")))
        .map(|heading| {
            automation
                .CreatePropertyConditionEx(
                    UIA_NamePropertyId,
                    &VARIANT::from(heading.as_str()),
                    PropertyConditionFlags_IgnoreCase,
                )
                .map(Some)
        })
        .collect::<windows::core::Result<_>>()
        .map_err(|_| ())?;
    let names = automation
        .CreateOrConditionFromNativeArray(&name_matches)
        .map_err(|_| ())?;
    let visible = automation
        .CreatePropertyCondition(UIA_IsOffscreenPropertyId, &VARIANT::from(false))
        .map_err(|_| ())?;
    let text = automation
        .CreatePropertyCondition(
            UIA_ControlTypePropertyId,
            &VARIANT::from(UIA_TextControlTypeId.0),
        )
        .map_err(|_| ())?;
    let condition = automation
        .CreateAndConditionFromNativeArray(&[Some(names), Some(visible), Some(text)])
        .map_err(|_| ())?;
    let matches = match root.FindAll(TreeScope_Descendants, &condition) {
        Ok(matches) => matches,
        // Windows can return S_OK and a null pointer for an empty search result.
        Err(error) if error.code().is_ok() => return Ok(None),
        Err(_) => return Err(()),
    };
    let count = matches.Length().map_err(|_| ())?;
    if !(0..=8).contains(&count) {
        return Err(());
    }

    let mut heading = None;
    for index in 0..count {
        let element = matches.GetElement(index).map_err(|_| ())?;
        let name = element.CurrentName().map_err(|_| ())?;
        if is_models_page_heading(&name.to_string()) && is_settings_heading(automation, &element) {
            if heading.replace(element).is_some() {
                return Err(());
            }
        }
    }
    Ok(heading)
}

unsafe fn is_settings_heading(automation: &IUIAutomation, heading: &IUIAutomationElement) -> bool {
    let Ok(dialog) = automation
        .ControlViewWalker()
        .and_then(|walker| walker.GetParentElement(heading))
    else {
        return false;
    };
    if dialog.CurrentControlType().ok() != Some(UIA_WindowControlTypeId) {
        return false;
    }
    dialog.CurrentName().ok().is_some_and(|name| {
        let name = name.to_string();
        name.eq_ignore_ascii_case("Settings") || name == "设置"
    })
}

unsafe fn visible_text_siblings(
    automation: &IUIAutomation,
    heading: &IUIAutomationElement,
    deadline: Instant,
) -> Option<Vec<String>> {
    let walker = automation.ControlViewWalker().ok()?;
    let cache = automation.CreateCacheRequest().ok()?;
    for property in [
        UIA_NamePropertyId,
        UIA_ControlTypePropertyId,
        UIA_IsOffscreenPropertyId,
    ] {
        cache.AddProperty(property).ok()?;
    }

    let mut names = vec![heading.CurrentName().ok()?.to_string()];
    let mut current = heading.clone();
    for _ in 0..MAX_SIBLINGS {
        if Instant::now() >= deadline {
            return None;
        }
        current = match walker.GetNextSiblingElementBuildCache(&current, &cache) {
            Ok(element) => element,
            // An absent next sibling is returned as S_OK with a null pointer.
            Err(error) if error.code().is_ok() => return Some(names),
            Err(_) => return None,
        };
        if current.CachedControlType().ok()? != UIA_TextControlTypeId
            || current.CachedIsOffscreen().ok()?.as_bool()
        {
            continue;
        }
        let name = current.CachedName().ok()?;
        if !name.is_empty() && name.len() <= MAX_TEXT_LENGTH {
            names.push(name.to_string());
        }
    }
    None
}
