#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(not(windows))]
compile_error!("hello-window is a Windows-only lab target");

#[cfg(windows)]
mod windows_app {
    use core::ffi::c_void;
    use core::ptr::{null, null_mut};

    type Handle = *mut c_void;
    type HInstance = Handle;
    type HWnd = Handle;
    type HBrush = Handle;
    type HCursor = Handle;
    type HIcon = Handle;
    type HMenu = Handle;
    type LParam = isize;
    type LResult = isize;
    type WParam = usize;

    const COLOR_WINDOW: usize = 5;
    const CS_HREDRAW: u32 = 0x0002;
    const CS_VREDRAW: u32 = 0x0001;
    const CW_USEDEFAULT: i32 = i32::MIN;
    const IDC_ARROW: *const u16 = 32512usize as *const u16;
    const SS_CENTER: u32 = 0x0001;
    const SW_SHOW: i32 = 5;
    const WM_DESTROY: u32 = 0x0002;
    const WS_CHILD: u32 = 0x4000_0000;
    const WS_OVERLAPPEDWINDOW: u32 = 0x00cf_0000;
    const WS_VISIBLE: u32 = 0x1000_0000;

    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    struct Message {
        hwnd: HWnd,
        message: u32,
        w_param: WParam,
        l_param: LParam,
        time: u32,
        point: Point,
        private: u32,
    }

    type WindowProcedure = Option<unsafe extern "system" fn(HWnd, u32, WParam, LParam) -> LResult>;

    #[repr(C)]
    struct WindowClass {
        style: u32,
        procedure: WindowProcedure,
        class_extra: i32,
        window_extra: i32,
        instance: HInstance,
        icon: HIcon,
        cursor: HCursor,
        background: HBrush,
        menu_name: *const u16,
        class_name: *const u16,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleW(module_name: *const u16) -> HInstance;
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn CreateWindowExW(
            extended_style: u32,
            class_name: *const u16,
            window_name: *const u16,
            style: u32,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            parent: HWnd,
            menu: HMenu,
            instance: HInstance,
            parameter: *const c_void,
        ) -> HWnd;
        fn DefWindowProcW(window: HWnd, message: u32, w_param: WParam, l_param: LParam) -> LResult;
        fn DispatchMessageW(message: *const Message) -> LResult;
        fn GetMessageW(message: *mut Message, window: HWnd, first: u32, last: u32) -> i32;
        fn LoadCursorW(instance: HInstance, cursor_name: *const u16) -> HCursor;
        fn PostQuitMessage(exit_code: i32);
        fn RegisterClassW(window_class: *const WindowClass) -> u16;
        fn ShowWindow(window: HWnd, command: i32) -> i32;
        fn TranslateMessage(message: *const Message) -> i32;
        fn UpdateWindow(window: HWnd) -> i32;
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(core::iter::once(0)).collect()
    }

    unsafe extern "system" fn window_procedure(
        window: HWnd,
        message: u32,
        w_param: WParam,
        l_param: LParam,
    ) -> LResult {
        if message == WM_DESTROY {
            // SAFETY: posting a quit message is valid on the owning GUI thread.
            unsafe { PostQuitMessage(0) };
            return 0;
        }
        // SAFETY: unhandled messages are delegated to the system procedure with
        // the exact values supplied by Windows.
        unsafe { DefWindowProcW(window, message, w_param, l_param) }
    }

    pub fn run() {
        let class_name = wide("MiniConLabHelloWindow");
        let title = wide("Hello World — MiniCon Lab");
        let static_class = wide("STATIC");
        let message_text = wide("Hello, world!");

        // SAFETY: all pointers below remain valid through registration/window
        // creation, and the message loop runs on this owning thread.
        unsafe {
            let instance = GetModuleHandleW(null());
            let window_class = WindowClass {
                style: CS_HREDRAW | CS_VREDRAW,
                procedure: Some(window_procedure),
                class_extra: 0,
                window_extra: 0,
                instance,
                icon: null_mut(),
                cursor: LoadCursorW(null_mut(), IDC_ARROW),
                background: (COLOR_WINDOW + 1) as HBrush,
                menu_name: null(),
                class_name: class_name.as_ptr(),
            };
            assert!(RegisterClassW(&window_class) != 0, "RegisterClassW failed");

            let window = CreateWindowExW(
                0,
                class_name.as_ptr(),
                title.as_ptr(),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                400,
                200,
                null_mut(),
                null_mut(),
                instance,
                null(),
            );
            assert!(!window.is_null(), "CreateWindowExW failed");

            let label = CreateWindowExW(
                0,
                static_class.as_ptr(),
                message_text.as_ptr(),
                WS_CHILD | WS_VISIBLE | SS_CENTER,
                25,
                45,
                335,
                60,
                window,
                null_mut(),
                instance,
                null(),
            );
            assert!(!label.is_null(), "STATIC control creation failed");
            ShowWindow(window, SW_SHOW);
            UpdateWindow(window);

            let mut message: Message = core::mem::zeroed();
            while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
}

fn main() {
    #[cfg(windows)]
    windows_app::run();
}
