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


def prepare(report, allow_close, closed_roles, observe_cloaked=False):
    require_hosted()  # Must precede all native DLL access.
    if not allow_close:
        raise Blocked('explicit known-setup-window close option required')
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
        (u, 'IsIconic', w.BOOL, [w.HWND]),
        (u, 'GetWindowRect', w.BOOL, [w.HWND, c.POINTER(w.RECT)]),
        (u, 'WindowFromPoint', w.HWND, [w.POINT]),
        (u, 'SetThreadDpiAwarenessContext', w.HANDLE, [w.HANDLE]),
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

        def visible_root_hit(target):
            previous_dpi = u.SetThreadDpiAwarenessContext(c.c_void_p(-4))
            if not previous_dpi:
                raise Blocked('physical-coordinate observation unavailable')
            report['observation_thread_dpi_restored'] = False
            try:
                bounds = w.RECT()
                if (u.GetForegroundWindow() != target or not u.IsWindowVisible(target)
                        or not u.GetWindowRect(target, c.byref(bounds))
                        or not 0 < bounds.right - bounds.left <= 16384
                        or not 0 < bounds.bottom - bounds.top <= 16384
                        or any(abs(value) > 131072 for value in
                               [bounds.left, bounds.top, bounds.right, bounds.bottom])):
                    return False
                point = w.POINT((bounds.left + bounds.right) // 2,
                                (bounds.top + bounds.bottom) // 2)
                hit = u.WindowFromPoint(point)
                matched = bool(hit and u.GetAncestor(hit, 2) == target)
                report['foreground_visible_root_hit'] = matched
                if matched:
                    report['target_desktop_binding'] = 'foreground-visible-root-hit'
                return matched
            finally:
                report['observation_thread_dpi_restored'] = bool(u.SetThreadDpiAwarenessContext(previous_dpi))
                if not report['observation_thread_dpi_restored']:
                    raise Blocked('observation thread DPI restoration failed')

        def contains_window(target):
            if role in ('search', 'start-menu'):
                matched = bool(u.GetForegroundWindow() == target and u.IsWindowVisible(target))
                if matched:
                    report['target_desktop_binding'] = 'default-input-foreground-system-panel'
                return matched
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
                report['target_desktop_binding'] = 'input-desktop-enumeration'
                return True
            if not completed or overflow:
                raise Blocked('bounded input-desktop window enumeration unavailable')
            report['input_desktop_enumeration_target_found'] = False
            return visible_root_hit(target)

        hwnd = u.GetForegroundWindow()
        foreground_start = time.monotonic()
        while not hwnd and time.monotonic() - foreground_start < 5:
            time.sleep(0.05)
            hwnd = u.GetForegroundWindow()
        report['foreground_wait_ms'] = round((time.monotonic() - foreground_start) * 1000)
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
        known = {
            'Microsoft.Windows.CloudExperienceHost_cw5n1h2txyewy':
                ('onboarding', ('System32', 'WWAHost.exe')),
            'MicrosoftWindows.Client.CBS_cw5n1h2txyewy':
                ('search', ('SystemApps', 'MicrosoftWindows.Client.CBS_cw5n1h2txyewy', 'SearchHost.exe')),
            'Microsoft.Windows.StartMenuExperienceHost_cw5n1h2txyewy':
                ('start-menu', ('SystemApps', 'Microsoft.Windows.StartMenuExperienceHost_cw5n1h2txyewy', 'StartMenuExperienceHost.exe')),
        }
        if code != 0 or family.value not in known:
            report['action'] = 'none-unrecognized-package'
            return
        role, relative_image = known[family.value]
        report['role'] = role
        if role in closed_roles:
            raise Blocked('known setup role reappeared; no repeated close')
        image = c.create_unicode_buffer(32768)
        length = w.DWORD(len(image))
        system_root = os.environ.get('SystemRoot')
        expected_image = Path(system_root).joinpath(*relative_image) if system_root else None

        def checked(name, value):
            report.setdefault('target_identity_checks', {})[name] = bool(value)
            return value

        if (not system_root or not k.QueryFullProcessImageNameW(handle, 0, image, c.byref(length))
                or not checked('system_image', os.path.normcase(image.value) == os.path.normcase(str(expected_image)))
                or not checked('same_session', session(pid.value) == session(k.GetCurrentProcessId()))
                or not checked('input_desktop_member', contains_window(hwnd))
                or not checked('process_alive', k.WaitForSingleObject(handle, 0) == 0x102)):
            raise Blocked('known setup image/session/lifetime mismatch')
        report['known_setup_identity_verified'] = True

        def presentation():
            # Metadata only. Never use cloaking to relax admission/close acknowledgement.
            state = {'exists': bool(u.IsWindow(hwnd)),
                     'style_visible': bool(u.IsWindowVisible(hwnd)),
                     'iconic': bool(u.IsIconic(hwnd)),
                     'is_foreground': u.GetForegroundWindow() == hwnd}
            try:
                dwm = c.WinDLL('dwmapi', use_last_error=True)
                read = dwm.DwmGetWindowAttribute
                read.restype = c.c_int32
                read.argtypes = [w.HWND, w.DWORD, c.c_void_p, w.DWORD]
                cloaked = w.DWORD()
                result = read(hwnd, 14, c.byref(cloaked), c.sizeof(cloaked))
                state['cloaked_hresult'] = f'0x{result & 0xffffffff:08x}'
                state['cloaked_status'] = 'observed' if result == 0 else 'unavailable'
                if result == 0:
                    state['cloaked_flags'] = cloaked.value
            except (OSError, AttributeError):
                state['cloaked_status'] = 'unavailable'
            return state

        report['presentation_before_close'] = presentation()
        if (observe_cloaked and role in ('search', 'start-menu')
                and report['presentation_before_close'].get('cloaked_flags') == 2):
            report['action'] = 'none-observed-shell-cloaked-panel'
            return  # Observation only; original physical court still owns every input admission.
        current_pid = w.DWORD()
        current_thread = u.GetWindowThreadProcessId(hwnd, c.byref(current_pid))
        if (u.GetForegroundWindow() != hwnd or not u.IsWindowVisible(hwnd)
                or current_pid.value != pid.value or current_thread != thread
                or k.WaitForSingleObject(handle, 0) != 0x102):
            raise Blocked('foreground changed before graceful close')
        if not u.PostMessageW(hwnd, 0x10, 0, 0):
            raise Blocked('known setup graceful close denied')
        report['close_requests'] = 1
        report['action'] = 'one-known-setup-window-WM_CLOSE'
        end = time.monotonic() + 5
        while time.monotonic() < end:
            if (not u.IsWindow(hwnd) or not u.IsWindowVisible(hwnd)) and u.GetForegroundWindow() != hwnd:
                report['known_window_no_longer_visible'] = True
                report['presentation_after_close'] = presentation()
                return
            time.sleep(0.05)
        report['presentation_after_close'] = presentation()
        raise Blocked('known setup window did not close within five seconds')
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
    parser.add_argument('--close-known-setup-windows', action='store_true')
    parser.add_argument('--observe-known-cloaked-panels', action='store_true')
    args = parser.parse_args()
    path = args.receipt.resolve()
    if path.exists():
        parser.error('preparation receipt must be fresh')
    path.parent.mkdir(parents=True, exist_ok=True)
    report = {'kind': 'hosted-arm64-default-preparation', 'source_sha': args.source_sha,
              'status': 'BLOCKED', 'native_input_qualified': False, 'close_requests': 0,
              'observe_known_cloaked_panels': args.observe_known_cloaked_panels,
              'force_kill': False, 'clipboard_or_input_used': False,
              'setup_windows_restored': False, 'restoration_scope': 'disposable VM only; no window restoration claim'}
    observations = []
    closed_roles = set()
    try:
        if len(args.source_sha) != 40 or any(ch not in '0123456789abcdef' for ch in args.source_sha):
            raise Blocked('exact lowercase source SHA required')
        for _ in range(4):
            observation = {'close_requests': 0}
            observations.append(observation)
            prepare(observation, args.close_known_setup_windows, closed_roles, args.observe_known_cloaked_panels)
            if observation['close_requests'] == 0:
                report['status'] = 'OBSERVED'
                break
            closed_roles.add(observation['role'])
        else:
            raise Blocked('known setup observation budget exhausted')
    except Blocked as error:
        report['reason'] = str(error)
    except Exception as error:
        report['reason'] = 'preparation error: ' + type(error).__name__
    report['observations'] = observations
    report['close_requests'] = sum(item['close_requests'] for item in observations)
    report['default_desktop_verified'] = bool(observations) and all(item.get('default_desktop_verified', False) for item in observations)
    for field in ('input_desktop_handle_closed', 'process_handle_closed', 'observation_thread_dpi_restored'):
        report[field] = all(item.get(field, True) for item in observations)
    path.write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(report))
    return 0 if report['status'] == 'OBSERVED' else 2


if __name__ == '__main__':
    raise SystemExit(main())
