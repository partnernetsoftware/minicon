#!/usr/bin/env python3
"""One explicit hosted ARM64 setup intervention. Owner: PRD 32. Never run locally."""
from __future__ import annotations

import argparse
import ctypes as c
from ctypes import wintypes as w
import json
import os
from pathlib import Path
import time

from run import Blocked, require_hosted


def prepare(report, allow_close):
    require_hosted()  # Must precede all native DLL access.
    if not allow_close:
        raise Blocked('explicit known-onboarding close option required')
    if os.environ.get('RUNNER_ARCH') != 'ARM64':
        raise Blocked('hosted ARM64 preparation required')
    u = c.WinDLL('user32', use_last_error=True)
    k = c.WinDLL('kernel32', use_last_error=True)
    window_callback = c.WINFUNCTYPE(w.BOOL, w.HWND, w.LPARAM)
    for dll, name, result, arguments in [
        (u, 'GetProcessWindowStation', w.HANDLE, []),
        (u, 'GetThreadDesktop', w.HANDLE, [w.DWORD]),
        (u, 'OpenInputDesktop', w.HANDLE, [w.DWORD, w.BOOL, w.DWORD]),
        (u, 'EnumDesktopWindows', w.BOOL, [w.HANDLE, window_callback, w.LPARAM]),
        (u, 'CloseDesktop', w.BOOL, [w.HANDLE]),
        (u, 'GetUserObjectInformationW', w.BOOL,
         [w.HANDLE, c.c_int, c.c_void_p, w.DWORD, c.POINTER(w.DWORD)]),
        (u, 'GetForegroundWindow', w.HWND, []),
        (u, 'GetAncestor', w.HWND, [w.HWND, w.UINT]),
        (u, 'GetWindowThreadProcessId', w.DWORD, [w.HWND, c.POINTER(w.DWORD)]),
        (u, 'GetClassNameW', c.c_int, [w.HWND, w.LPWSTR, c.c_int]),
        (u, 'IsWindowVisible', w.BOOL, [w.HWND]),
        (u, 'IsWindow', w.BOOL, [w.HWND]),
        (u, 'PostMessageW', w.BOOL, [w.HWND, w.UINT, w.WPARAM, w.LPARAM]),
        (k, 'GetCurrentThreadId', w.DWORD, []),
        (k, 'GetCurrentProcessId', w.DWORD, []),
        (k, 'ProcessIdToSessionId', w.BOOL, [w.DWORD, c.POINTER(w.DWORD)]),
        (k, 'OpenProcess', w.HANDLE, [w.DWORD, w.BOOL, w.DWORD]),
        (k, 'QueryFullProcessImageNameW', w.BOOL,
         [w.HANDLE, w.DWORD, w.LPWSTR, c.POINTER(w.DWORD)]),
        (k, 'GetPackageFamilyName', w.LONG, [w.HANDLE, c.POINTER(w.DWORD), w.LPWSTR]),
        (k, 'WaitForSingleObject', w.DWORD, [w.HANDLE, w.DWORD]),
        (k, 'CloseHandle', w.BOOL, [w.HANDLE]),
    ]:
        function = getattr(dll, name)
        function.restype, function.argtypes = result, arguments

    def object_name(handle):
        value = c.create_unicode_buffer(512)
        size = w.DWORD()
        if not handle or not u.GetUserObjectInformationW(handle, 2, value, c.sizeof(value), c.byref(size)):
            raise Blocked('desktop identity unavailable')
        return value.value.casefold()

    def session(pid):
        value = w.DWORD()
        if not k.ProcessIdToSessionId(pid, c.byref(value)):
            raise Blocked('process session unavailable')
        return value.value

    if (object_name(u.GetProcessWindowStation()) != 'winsta0'
            or object_name(u.GetThreadDesktop(k.GetCurrentThreadId())) != 'default'):
        raise Blocked('interactive Default calling desktop required')
    desktop = u.OpenInputDesktop(0, False, 0x41)  # READOBJECTS | ENUMERATE, no switch
    if not desktop:
        raise Blocked('Default input desktop unavailable')
    handle = None
    report['input_desktop_handle_closed'] = False
    try:
        if object_name(desktop) != 'default':
            raise Blocked('non-secure Default input desktop required')
        report['default_desktop_verified'] = True

        def contains_window(target):
            count, found, overflow = 0, False, False

            @window_callback
            def visit(candidate, _):
                nonlocal count, found, overflow
                if count >= 256:
                    overflow = True
                    return False
                count += 1
                if candidate == target:
                    found = True
                    return False
                return True

            completed = u.EnumDesktopWindows(desktop, visit, 0)
            if found:
                report['target_on_input_desktop_verified'] = True
                return True
            if not completed or overflow:
                raise Blocked('bounded input-desktop window enumeration unavailable')
            return False

        hwnd = u.GetForegroundWindow()
        if not hwnd:
            report['action'] = 'none-no-foreground'
            return
        pid = w.DWORD()
        thread = u.GetWindowThreadProcessId(hwnd, c.byref(pid))
        class_name = c.create_unicode_buffer(128)
        if (not thread or not u.GetClassNameW(hwnd, class_name, len(class_name))
                or u.GetAncestor(hwnd, 2) != hwnd):
            raise Blocked('foreground root identity unavailable')
        if class_name.value != 'Windows.UI.Core.CoreWindow':
            report['action'] = 'none-unrecognized-foreground'
            return
        handle = k.OpenProcess(0x101000, False, pid.value)  # query + synchronize only
        if not handle:
            raise Blocked('foreground limited-query handle unavailable')
        report['process_handle_closed'] = False
        family = c.create_unicode_buffer(512)
        length = w.DWORD(len(family))
        code = k.GetPackageFamilyName(handle, c.byref(length), family)
        if code != 0 or family.value != 'Microsoft.Windows.CloudExperienceHost_cw5n1h2txyewy':
            report['action'] = 'none-unrecognized-package'
            return
        image = c.create_unicode_buffer(32768)
        length = w.DWORD(len(image))
        system_root = os.environ.get('SystemRoot')

        def checked(name, value):
            report.setdefault('target_identity_checks', {})[name] = bool(value)
            return value

        if (not system_root or not k.QueryFullProcessImageNameW(handle, 0, image, c.byref(length))
                or not checked('system_image', os.path.normcase(image.value) == os.path.normcase(str(Path(system_root) / 'System32' / 'WWAHost.exe')))
                or not checked('same_session', session(pid.value) == session(k.GetCurrentProcessId()))
                or not checked('input_desktop_member', contains_window(hwnd))
                or not checked('process_alive', k.WaitForSingleObject(handle, 0) == 0x102)):
            raise Blocked('known onboarding image/session/lifetime mismatch')
        report['known_onboarding_identity_verified'] = True
        current_pid = w.DWORD()
        current_thread = u.GetWindowThreadProcessId(hwnd, c.byref(current_pid))
        if (u.GetForegroundWindow() != hwnd or not u.IsWindowVisible(hwnd)
                or current_pid.value != pid.value or current_thread != thread
                or k.WaitForSingleObject(handle, 0) != 0x102):
            raise Blocked('foreground changed before graceful close')
        if not u.PostMessageW(hwnd, 0x10, 0, 0):
            raise Blocked('known onboarding graceful close denied')
        report['close_requests'] = 1
        report['action'] = 'one-known-onboarding-WM_CLOSE'
        end = time.monotonic() + 5
        while time.monotonic() < end:
            if (not u.IsWindow(hwnd) or not u.IsWindowVisible(hwnd)) and u.GetForegroundWindow() != hwnd:
                report['known_window_no_longer_visible'] = True
                return
            time.sleep(0.05)
        raise Blocked('known onboarding did not close within five seconds')
    finally:
        if handle:
            report['process_handle_closed'] = bool(k.CloseHandle(handle))
        report['input_desktop_handle_closed'] = bool(u.CloseDesktop(desktop))
        if not report['input_desktop_handle_closed'] or not report.get('process_handle_closed', True):
            raise Blocked('preparation handle cleanup failed')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--receipt', type=Path, required=True)
    parser.add_argument('--source-sha', required=True)
    parser.add_argument('--close-known-onboarding', action='store_true')
    args = parser.parse_args()
    path = args.receipt.resolve()
    if path.exists():
        parser.error('preparation receipt must be fresh')
    path.parent.mkdir(parents=True, exist_ok=True)
    report = {'kind': 'hosted-arm64-default-preparation', 'source_sha': args.source_sha,
              'status': 'BLOCKED', 'native_input_qualified': False, 'close_requests': 0,
              'force_kill': False, 'clipboard_or_input_used': False,
              'onboarding_restored': False, 'restoration_scope': 'disposable VM only; no onboarding restoration claim'}
    try:
        if len(args.source_sha) != 40 or any(ch not in '0123456789abcdef' for ch in args.source_sha):
            raise Blocked('exact lowercase source SHA required')
        prepare(report, args.close_known_onboarding)
        report['status'] = 'OBSERVED'
    except Blocked as error:
        report['reason'] = str(error)
    except Exception as error:
        report['reason'] = 'preparation error: ' + type(error).__name__
    path.write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(report))
    return 0 if report['status'] == 'OBSERVED' else 2


if __name__ == '__main__':
    raise SystemExit(main())
