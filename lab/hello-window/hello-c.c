#include <windows.h>

static LRESULT CALLBACK window_procedure(
    HWND window, UINT message, WPARAM w_param, LPARAM l_param) {
  if (message == WM_DESTROY) {
    PostQuitMessage(0);
    return 0;
  }
  return DefWindowProcW(window, message, w_param, l_param);
}

int WINAPI wWinMain(HINSTANCE instance, HINSTANCE previous, PWSTR command,
                    int show_command) {
  (void)previous;
  (void)command;

  static const wchar_t class_name[] = L"MiniConLabPureCHelloWindow";
  WNDCLASSW window_class = {0};
  window_class.lpfnWndProc = window_procedure;
  window_class.hInstance = instance;
  window_class.hCursor = LoadCursorW(NULL, IDC_ARROW);
  window_class.hbrBackground = (HBRUSH)(COLOR_WINDOW + 1);
  window_class.lpszClassName = class_name;
  if (!RegisterClassW(&window_class)) {
    return 10;
  }

  HWND window = CreateWindowExW(
      0, class_name, L"Hello World - MiniCon pure C control",
      WS_OVERLAPPEDWINDOW, CW_USEDEFAULT, CW_USEDEFAULT, 400, 200, NULL, NULL,
      instance, NULL);
  if (!window) {
    return 11;
  }
  if (!CreateWindowExW(0, L"STATIC", L"Hello, world!",
                       WS_CHILD | WS_VISIBLE | SS_CENTER, 25, 45, 335, 60,
                       window, NULL, instance, NULL)) {
    return 12;
  }

  ShowWindow(window, show_command);
  UpdateWindow(window);
  MSG message;
  int result;
  while ((result = GetMessageW(&message, NULL, 0, 0)) > 0) {
    TranslateMessage(&message);
    DispatchMessageW(&message);
  }
  return result < 0 ? 13 : (int)message.wParam;
}
