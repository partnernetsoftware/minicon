#!/usr/bin/env python3
"""Allocate the ARM64 hosted input court; owner: PRD 32. Never run locally."""
from __future__ import annotations

import argparse
import ctypes as c
from ctypes import wintypes as w
import hashlib
import json
import os
from pathlib import Path
import secrets
import subprocess
import sys

from run import Blocked, OwnedProcess, require_hosted


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--source-sha', required=True)
    parser.add_argument('--desktop-receipt', type=Path, required=True)
    args = parser.parse_args()
    receipt_path = args.desktop_receipt.resolve()
    if receipt_path.exists():
        parser.error('desktop receipt must be fresh')
    receipt_path.parent.mkdir(parents=True, exist_ok=True)
    report = {'kind': 'hosted-allocated-input-desktop', 'schema': 1,
              'status': 'BLOCKED', 'source_sha': args.source_sha,
              'native_input_qualified': False, 'controller_exit': None,
              'scope': 'Existing physical controls on this allocated hosted desktop and measured image only',
              'original_desktop_restored': True, 'thread_desktop_restored': True,
              'owned_desktop_handle_closed': True, 'original_handle_closed': True,
              'controller_reaped': True, 'controller_job_closed': True}
    user = kernel = child = original = allocated = thread_original = None
    original_name = None
    switched = False
    code = 2

    def desktop_name(handle):
        name = c.create_unicode_buffer(512)
        needed = w.DWORD()
        if not user.GetUserObjectInformationW(handle, 2, name, c.sizeof(name), c.byref(needed)):
            raise Blocked('bounded desktop identity readback unavailable')
        return name.value

    def input_matches(expected):
        current = user.OpenInputDesktop(0, False, 0x101)
        if not current:
            return False
        try:
            return desktop_name(current).casefold() == expected.casefold()
        finally:
            if not user.CloseDesktop(current):
                raise Blocked('input-desktop readback handle close failed')

    try:
        require_hosted()  # Must precede every native DLL call and allocation.
        if len(args.source_sha) != 40 or any(ch not in '0123456789abcdef' for ch in args.source_sha):
            raise Blocked('exact lowercase source SHA required')
        user = c.WinDLL('user32', use_last_error=True)
        kernel = c.WinDLL('kernel32', use_last_error=True)
        for dll, name, result, types in [
            (user, 'OpenInputDesktop', w.HANDLE, [w.DWORD, w.BOOL, w.DWORD]),
            (user, 'CreateDesktopW', w.HANDLE, [w.LPCWSTR, w.LPCWSTR, c.c_void_p, w.DWORD, w.DWORD, c.c_void_p]),
            (user, 'SwitchDesktop', w.BOOL, [w.HANDLE]),
            (user, 'CloseDesktop', w.BOOL, [w.HANDLE]),
            (user, 'GetThreadDesktop', w.HANDLE, [w.DWORD]),
            (user, 'SetThreadDesktop', w.BOOL, [w.HANDLE]),
            (user, 'GetProcessWindowStation', w.HANDLE, []),
            (user, 'GetUserObjectInformationW', w.BOOL, [w.HANDLE, c.c_int, c.c_void_p, w.DWORD, c.POINTER(w.DWORD)]),
            (kernel, 'GetCurrentThreadId', w.DWORD, []),
        ]:
            function = getattr(dll, name)
            function.restype, function.argtypes = result, types
        if desktop_name(user.GetProcessWindowStation()).casefold() != 'winsta0':
            raise Blocked('interactive hosted window station required')
        report['interactive_window_station'] = True
        thread_original = user.GetThreadDesktop(kernel.GetCurrentThreadId())
        original = user.OpenInputDesktop(0, False, 0x101)
        if not original or not thread_original:
            raise Blocked('original hosted desktop capture unavailable')
        report['original_handle_closed'] = False
        original_name = desktop_name(original)
        if original_name.casefold() != 'default' or desktop_name(thread_original).casefold() != 'default':
            raise Blocked('only the default non-secure hosted desktop may be allocated from')
        name = 'minicon-court-' + secrets.token_hex(16)
        allocated = user.CreateDesktopW(name, None, None, 0, 0x1FF, None)
        if not allocated:
            raise Blocked('private hosted desktop allocation unavailable')
        report['owned_desktop_handle_closed'] = False
        report['thread_desktop_restored'] = False
        report['original_desktop_restored'] = False
        if not user.SwitchDesktop(allocated):
            raise Blocked('private hosted input-desktop switch unavailable')
        switched = True
        if not input_matches(name):
            raise Blocked('allocated input desktop identity did not match')
        report['allocated_input_desktop_verified'] = True
        child = OwnedProcess(Path(sys.executable),
                             ['-B', '-X', 'utf8', str(Path(__file__).with_name('run.py')),
                              '--binary', str(args.binary.resolve(strict=True)),
                              '--output', str(args.output.resolve()), '--source-sha', args.source_sha],
                             os.environ.copy(), desktop='WinSta0\\' + name)
        report['controller_reaped'] = False
        report['controller_job_closed'] = False
        code = child.wait(timeout=300)
        report['controller_exit'] = code
        report['controller_reaped'] = True
        inner_receipt = args.output.resolve() / 'receipt.json'
        inner = json.loads(inner_receipt.read_text(encoding='utf-8'))
        if (inner['source_sha'] != args.source_sha or inner['binary_sha256'] !=
                hashlib.sha256(args.binary.resolve(strict=True).read_bytes()).hexdigest()
                or (code == 0 and inner['status'] != 'passed')):
            raise Blocked('inner physical receipt did not match source image or exit')
        report['inner_receipt_sha256'] = hashlib.sha256(inner_receipt.read_bytes()).hexdigest()
        report['status'] = 'passed' if code == 0 else 'BLOCKED' if code == 2 else 'failed'
    except Blocked as error:
        report['reason'] = str(error)
    except subprocess.TimeoutExpired:
        report['reason'] = 'allocated desktop controller deadline expired'
    except Exception as error:
        report['reason'] = 'allocated desktop controller error: ' + type(error).__name__
    finally:
        if child is not None:
            try:
                if child.poll() is None:
                    child.k.TerminateJobObject(child.job, 1)
                    child.wait(timeout=5)
                    report['forced_owned_controller_cleanup'] = True
                report['controller_reaped'] = child.poll() is not None
            except (Blocked, subprocess.TimeoutExpired):
                report['controller_reaped'] = False
            finally:
                child.close()
                report['controller_job_closed'] = True
        if user is not None:
            try:
                if allocated is not None:
                    # Restore input even if the allocation failed before switch
                    # readback. Never leave a failed partial setup active.
                    report['original_desktop_restored'] = bool(
                        (not switched or user.SwitchDesktop(original)) and input_matches(original_name))
                    current_thread = user.GetThreadDesktop(kernel.GetCurrentThreadId())
                    if desktop_name(current_thread).casefold() != 'default':
                        user.SetThreadDesktop(thread_original)
                    current_thread = user.GetThreadDesktop(kernel.GetCurrentThreadId())
                    report['thread_desktop_restored'] = desktop_name(current_thread).casefold() == 'default'
            except (Blocked, OSError):
                report['original_desktop_restored'] = False
                report['thread_desktop_restored'] = False
            finally:
                if allocated is not None:
                    report['owned_desktop_handle_closed'] = bool(user.CloseDesktop(allocated))
                if original is not None:
                    report['original_handle_closed'] = bool(user.CloseDesktop(original))
        report['input_desktop_was_switched'] = switched
        cleanup = ('original_desktop_restored', 'thread_desktop_restored',
                   'owned_desktop_handle_closed', 'original_handle_closed',
                   'controller_reaped', 'controller_job_closed')
        if not all(report[key] for key in cleanup) or report.get('forced_owned_controller_cleanup'):
            report['status'] = 'BLOCKED'
            report['cleanup_incomplete'] = True
        report['native_input_qualified'] = (report['status'] == 'passed')
        receipt_path.write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(report))
    return 0 if report['status'] == 'passed' else 2 if report['status'] == 'BLOCKED' else 1


if __name__ == '__main__':
    raise SystemExit(main())
