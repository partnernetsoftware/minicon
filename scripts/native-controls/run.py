#!/usr/bin/env python3
"""Isolated hosted Windows controls court. Owners: PRD 25.02 / 31.01 / 32.

Reuses browser-workspace-smoke.py's isolated launch/public observations and
tests/minicon_blackbox.rs's real Pinyin activation. No send-ui-keys/ime/mouse.
Requires a disposable GitHub-hosted interactive desktop and registered Pinyin.
Refuses local execution before accessing clipboard, focus or native input.
No account login/inference is attempted. PNGs need human pixel review.
"""
from __future__ import annotations

import argparse
import ctypes as c
from ctypes import wintypes as w
import hashlib
import http.server
import json
import os
from pathlib import Path
import secrets
import struct
import subprocess
import sys
import tempfile
import threading
import time
import urllib.parse
import zlib

from clipboard_snapshot import ClipboardError, ClipboardSnapshot


class Blocked(RuntimeError):
    pass


class KeyboardInput(c.Structure):
    _fields_ = [("vk", w.WORD), ("scan", w.WORD), ("flags", w.DWORD),
                ("time", w.DWORD), ("extra", c.c_size_t)]


class MouseInput(c.Structure):
    _fields_ = [("dx", w.LONG), ("dy", w.LONG), ("data", w.DWORD),
                ("flags", w.DWORD), ("time", w.DWORD), ("extra", c.c_size_t)]


class InputUnion(c.Union):
    _fields_ = [("keyboard", KeyboardInput), ("mouse", MouseInput)]


class Input(c.Structure):
    _anonymous_ = ("payload",)
    _fields_ = [("kind", w.DWORD), ("payload", InputUnion)]


class GuiThreadInfo(c.Structure):
    _fields_ = [("size", w.DWORD), ("flags", w.DWORD),
                *[(name, w.HWND) for name in
                  ("active", "focus", "capture", "menu", "move", "caret")],
                ("caret_rect", w.RECT)]


class MonitorInfo(c.Structure):
    _fields_ = [("size", w.DWORD), ("monitor", w.RECT),
                ("work", w.RECT), ("flags", w.DWORD)]


class StartupInfo(c.Structure):
    _fields_ = [("size", w.DWORD), ("reserved", w.LPWSTR), ("desktop", w.LPWSTR),
                ("title", w.LPWSTR), *[(name, w.DWORD) for name in
                 ("x", "y", "cx", "cy", "chars_x", "chars_y", "fill", "flags")],
                ("show", w.WORD), ("reserved_size", w.WORD), ("reserved_ptr", c.c_void_p),
                ("stdin", w.HANDLE), ("stdout", w.HANDLE), ("stderr", w.HANDLE)]


class ProcessInfo(c.Structure):
    _fields_ = [("process", w.HANDLE), ("thread", w.HANDLE),
                ("pid", w.DWORD), ("tid", w.DWORD)]


class JobBasic(c.Structure):
    _fields_ = [("process_time", c.c_int64), ("job_time", c.c_int64),
                ("flags", w.DWORD), ("min_ws", c.c_size_t), ("max_ws", c.c_size_t),
                ("active", w.DWORD), ("affinity", c.c_size_t),
                ("priority", w.DWORD), ("scheduling", w.DWORD)]


class JobLimits(c.Structure):
    _fields_ = [("basic", JobBasic), ("io", c.c_uint64 * 6),
                *[(name, c.c_size_t) for name in ("process_mem", "job_mem", "peak_process", "peak_job")]]


class OwnedProcess:
    """Assign suspended GUI to kill-on-close Job before it can spawn children."""
    def __init__(self, binary, arguments, env, desktop=None):
        self.k = c.WinDLL("kernel32", use_last_error=True)
        for name, result, arguments_types in [
            ("CreateJobObjectW", w.HANDLE, [c.c_void_p, w.LPCWSTR]),
            ("SetInformationJobObject", w.BOOL, [w.HANDLE, c.c_int, c.c_void_p, w.DWORD]),
            ("AssignProcessToJobObject", w.BOOL, [w.HANDLE, w.HANDLE]),
            ("TerminateJobObject", w.BOOL, [w.HANDLE, w.UINT]),
            ("CreateProcessW", w.BOOL, [w.LPCWSTR, w.LPWSTR, c.c_void_p, c.c_void_p,
                w.BOOL, w.DWORD, c.c_void_p, w.LPCWSTR, c.POINTER(StartupInfo), c.POINTER(ProcessInfo)]),
            ("ResumeThread", w.DWORD, [w.HANDLE]),
            ("TerminateProcess", w.BOOL, [w.HANDLE, w.UINT]),
            ("WaitForSingleObject", w.DWORD, [w.HANDLE, w.DWORD]),
            ("GetExitCodeProcess", w.BOOL, [w.HANDLE, c.POINTER(w.DWORD)]),
            ("CloseHandle", w.BOOL, [w.HANDLE]),
        ]:
            function = getattr(self.k, name)
            function.restype, function.argtypes = result, arguments_types
        self.job = self.k.CreateJobObjectW(None, None)
        self.info = ProcessInfo()
        limits = JobLimits(basic=JobBasic(flags=0x2000))  # KILL_ON_JOB_CLOSE
        try:
            if not self.job or not self.k.SetInformationJobObject(self.job, 9, c.byref(limits), c.sizeof(limits)):
                raise Blocked("owned kill-on-close Job unavailable")
            startup = StartupInfo(size=c.sizeof(StartupInfo), desktop=desktop)
            command = c.create_unicode_buffer(subprocess.list2cmdline([str(binary), *arguments]))
            environment = c.create_unicode_buffer("\0".join(f"{key}={value}" for key, value in
                                                          sorted(env.items(), key=lambda item: item[0].upper())) + "\0\0")
            if not self.k.CreateProcessW(str(binary), command, None, None, False,
                    0x08000404, environment, None, c.byref(startup), c.byref(self.info)):
                raise Blocked("owned suspended GUI launch failed")
            if not self.k.AssignProcessToJobObject(self.job, self.info.process):
                raise Blocked("owned GUI Job assignment failed")
            if self.k.ResumeThread(self.info.thread) == 0xFFFFFFFF:
                raise Blocked("owned GUI resume failed")
            self.pid = self.info.pid
        except Exception:
            if self.info.process:
                self.k.TerminateProcess(self.info.process, 1)
                self.k.WaitForSingleObject(self.info.process, 2000)
            self.close()
            raise
        finally:
            if self.info.thread:
                self.k.CloseHandle(self.info.thread)
                self.info.thread = None

    def poll(self):
        if self.k.WaitForSingleObject(self.info.process, 0) == 0x102:
            return None
        code = w.DWORD()
        if not self.k.GetExitCodeProcess(self.info.process, c.byref(code)):
            raise Blocked("owned GUI exit observation failed")
        return code.value

    def wait(self, timeout):
        if self.k.WaitForSingleObject(self.info.process, int(timeout * 1000)) != 0:
            raise subprocess.TimeoutExpired("owned GUI", timeout)
        return self.poll()

    def close(self):
        if self.job:
            self.k.CloseHandle(self.job)
            self.job = None
        if self.info.process:
            self.k.CloseHandle(self.info.process)
            self.info.process = None


def require_hosted():
    if (sys.platform != "win32" or os.environ.get("GITHUB_ACTIONS") != "true"
            or os.environ.get("RUNNER_ENVIRONMENT") != "github-hosted"):
        raise Blocked("native input requires the disposable GitHub-hosted court")


class Native:
    def __init__(self):
        require_hosted()
        self.u = c.WinDLL("user32", use_last_error=True)
        self.g = c.WinDLL("gdi32", use_last_error=True)
        self.k = c.WinDLL("kernel32", use_last_error=True)
        self.i = c.WinDLL("imm32", use_last_error=True)
        for dll, name, restype, args in [
            (self.u, "GetForegroundWindow", w.HWND, []),
            (self.u, "SetForegroundWindow", w.BOOL, [w.HWND]),
            (self.u, "SetWindowPos", w.BOOL, [w.HWND, w.HWND, c.c_int, c.c_int, c.c_int, c.c_int, w.UINT]),
            (self.u, "IsWindowVisible", w.BOOL, [w.HWND]),
            (self.u, "IsWindow", w.BOOL, [w.HWND]),
            (self.u, "IsChild", w.BOOL, [w.HWND, w.HWND]),
            (self.u, "GetWindowThreadProcessId", w.DWORD, [w.HWND, c.POINTER(w.DWORD)]),
            (self.u, "GetClassNameW", c.c_int, [w.HWND, w.LPWSTR, c.c_int]),
            (self.u, "GetWindowRect", w.BOOL, [w.HWND, c.POINTER(w.RECT)]),
            (self.u, "MonitorFromWindow", w.HANDLE, [w.HWND, w.DWORD]),
            (self.u, "GetMonitorInfoW", w.BOOL, [w.HANDLE, c.POINTER(MonitorInfo)]),
            (self.u, "GetKeyboardLayout", c.c_void_p, [w.DWORD]),
            (self.u, "GetKeyboardLayoutList", c.c_int, [c.c_int, c.POINTER(c.c_void_p)]),
            (self.u, "GetGUIThreadInfo", w.BOOL, [w.DWORD, c.POINTER(GuiThreadInfo)]),
            (self.u, "GetAsyncKeyState", c.c_short, [c.c_int]),
            (self.u, "SendInput", w.UINT, [w.UINT, c.POINTER(Input), c.c_int]),
            (self.u, "ClientToScreen", w.BOOL, [w.HWND, c.POINTER(w.POINT)]),
            (self.u, "GetCursorPos", w.BOOL, [c.POINTER(w.POINT)]),
            (self.u, "SetCursorPos", w.BOOL, [c.c_int, c.c_int]),
            (self.u, "GetClientRect", w.BOOL, [w.HWND, c.POINTER(w.RECT)]),
            (self.u, "WindowFromPoint", w.HWND, [w.POINT]),
            (self.u, "GetAncestor", w.HWND, [w.HWND, w.UINT]),
            (self.u, "GetDC", w.HDC, [w.HWND]),
            (self.u, "ReleaseDC", c.c_int, [w.HWND, w.HDC]),
            (self.u, "SendMessageTimeoutW", c.c_ssize_t,
             [w.HWND, w.UINT, w.WPARAM, w.LPARAM, w.UINT, w.UINT, c.POINTER(c.c_size_t)]),
            (self.u, "OpenClipboard", w.BOOL, [w.HWND]),
            (self.u, "CloseClipboard", w.BOOL, []),
            (self.u, "EmptyClipboard", w.BOOL, []),
            (self.u, "GetClipboardData", w.HANDLE, [w.UINT]),
            (self.u, "SetClipboardData", w.HANDLE, [w.UINT, w.HANDLE]),
            (self.k, "GlobalAlloc", w.HGLOBAL, [w.UINT, c.c_size_t]),
            (self.k, "GlobalLock", c.c_void_p, [w.HGLOBAL]),
            (self.k, "GlobalUnlock", w.BOOL, [w.HGLOBAL]),
            (self.k, "GlobalFree", w.HGLOBAL, [w.HGLOBAL]),
            (self.k, "GlobalSize", c.c_size_t, [w.HGLOBAL]),
            (self.k, "OpenProcess", w.HANDLE, [w.DWORD, w.BOOL, w.DWORD]),
            (self.k, "QueryFullProcessImageNameW", w.BOOL,
             [w.HANDLE, w.DWORD, w.LPWSTR, c.POINTER(w.DWORD)]),
            (self.k, "IsProcessInJob", w.BOOL, [w.HANDLE, w.HANDLE, c.POINTER(w.BOOL)]),
            (self.k, "GetPackageFamilyName", w.LONG, [w.HANDLE, c.POINTER(w.DWORD), w.LPWSTR]),
            (self.k, "CloseHandle", w.BOOL, [w.HANDLE]),
            (self.g, "CreateCompatibleDC", w.HDC, [w.HDC]),
            (self.g, "CreateCompatibleBitmap", w.HBITMAP, [w.HDC, c.c_int, c.c_int]),
            (self.g, "SelectObject", w.HANDLE, [w.HDC, w.HANDLE]),
            (self.g, "DeleteObject", w.BOOL, [w.HANDLE]),
            (self.g, "DeleteDC", w.BOOL, [w.HDC]),
            (self.g, "BitBlt", w.BOOL,
             [w.HDC, c.c_int, c.c_int, c.c_int, c.c_int, w.HDC, c.c_int, c.c_int, w.DWORD]),
            (self.g, "GetDIBits", c.c_int,
             [w.HDC, w.HBITMAP, w.UINT, w.UINT, c.c_void_p, c.c_void_p, w.UINT]),
            (self.i, "ImmGetDefaultIMEWnd", w.HWND, [w.HWND]),
        ]:
            function = getattr(dll, name)
            function.restype, function.argtypes = restype, args
        self.u.SetProcessDpiAwarenessContext.argtypes = [c.c_void_p]
        if not self.u.SetProcessDpiAwarenessContext(c.c_void_p(-4)):
            raise Blocked("per-monitor DPI awareness unavailable")
        self.clipboard_dirty = False
        self.clipboard_diagnostics = {"changed": False}
        self.clipboard_snapshot = ClipboardSnapshot()
        try:
            # Complete preflight/deep copy before GUI launch or any SendInput.
            # An OleGetClipboard proxy is not an independently owned snapshot.
            self.clipboard_snapshot.capture()
        except ClipboardError as error:
            raise Blocked(str(error)) from error
        self.clipboard_diagnostics.update(self.clipboard_snapshot.diagnostics)
        self.original_foreground = self.u.GetForegroundWindow()
        self.pointer = w.POINT()
        self.u.GetCursorPos(c.byref(self.pointer))
        self.hwnd = None
        self.owned_pid = None
        self.owned_job = None
        self.presentation_topmost = False
        self.presentation_restored = True
        self.focus_observations = []
        self.tsf_profile = None
        self.tsf_profile_restored = True
        self.pressed = set()
        self.original_layout = None
        self.thread = None
        if any(self.u.GetAsyncKeyState(vk) & 0x8000 for vk in
               (0x01, 0x02, 0x10, 0x11, 0x12, 0x5B, 0x5C)):
            raise Blocked("release mouse/modifier keys before starting court")

    def attach(self, pid, job):
        self.owned_pid, self.owned_job = pid, job
        handles = []
        callback_type = c.WINFUNCTYPE(w.BOOL, w.HWND, w.LPARAM)

        @callback_type
        def visit(hwnd, _):
            owner = w.DWORD()
            self.u.GetWindowThreadProcessId(hwnd, c.byref(owner))
            if owner.value == pid and self.u.IsWindowVisible(hwnd):
                handles.append(hwnd)
            return True

        self.u.EnumWindows(visit, 0)
        if len(handles) != 1:
            raise Blocked("expected one visible top-level window owned by this GUI PID")
        self.hwnd = handles[0]
        self.fit_work_area()
        self.thread = self.u.GetWindowThreadProcessId(self.hwnd, None)
        self.original_layout = self.u.GetKeyboardLayout(self.thread)
        requested = bool(self.u.SetForegroundWindow(self.hwnd))
        self.activation = "SetForegroundWindow-or-initial-activation"
        if self.u.GetForegroundWindow() != self.hwnd and not requested:
            # A real user can activate an owned visible window with one click
            # even when Windows refuses a programmatic foreground request.
            # This path is confined to the disposable hosted court. Resolve the
            # actual window under the point immediately before the input;
            # never click an overlaid/foreign window or fight for focus.
            # Present only our own window above overlapping setup windows,
            # without activating it. Foreground still requires the real click.
            owner = w.DWORD()
            self.u.GetWindowThreadProcessId(self.hwnd, c.byref(owner))
            if owner.value != pid or not self.u.SetWindowPos(self.hwnd, None, 0, 0, 0, 0, 0x53):
                raise Blocked("owned window could not be presented without activation")
            rect = w.RECT()
            if not self.u.GetClientRect(self.hwnd, c.byref(rect)) or rect.right < 2 or rect.bottom < 2:
                raise Blocked("owned activation client bounds unavailable")
            point = w.POINT(rect.right // 2, rect.bottom // 2)
            if not self.u.ClientToScreen(self.hwnd, c.byref(point)):
                raise Blocked("owned activation point unavailable")
            if not self.u.SetCursorPos(point.x, point.y):
                raise Blocked("owned activation pointer move unavailable")
            hit = self.u.WindowFromPoint(point)
            if not self.owned_hit(hit):
                self.record_focus("normal-activation-point-obscured", hit, point)
                # One owned-window presentation adjustment only. Do not close,
                # activate or click the hosted image's overlapping setup UI.
                if not self.u.SetWindowPos(self.hwnd, w.HWND(-1), 0, 0, 0, 0, 0x53):
                    raise Blocked("owned topmost presentation unavailable; no input sent")
                self.presentation_topmost = True
                self.presentation_restored = False
                hit = self.u.WindowFromPoint(point)
            if not self.owned_hit(hit):
                self.record_focus("activation-point-obscured", hit, point)
                raise Blocked("owned activation point is obscured; no input sent")
            pair = (Input * 2)(Input(kind=0, mouse=MouseInput(flags=2)),
                               Input(kind=0, mouse=MouseInput(flags=4)))
            if self.u.SendInput(2, pair, c.sizeof(Input)) != 2:
                release = Input(kind=0, mouse=MouseInput(flags=4))
                self.u.SendInput(1, c.byref(release), c.sizeof(Input))
                raise Blocked("owned activation click was not fully accepted")
            self.activation = ("single-verified-owned-client-click-topmost" if self.presentation_topmost
                               else "single-verified-owned-client-click")
        end = time.monotonic() + 3
        while self.u.GetForegroundWindow() != self.hwnd:
            if time.monotonic() >= end:
                self.record_focus("initial-foreground-unconfirmed")
                raise Blocked("owned GUI foreground activation unconfirmed after 3s "
                              f"(SetForegroundWindow requested={requested})")
            time.sleep(0.05)

    def fit_work_area(self):
        """Keep this owned fixture's hit targets above the actual taskbar."""
        owner = w.DWORD()
        self.u.GetWindowThreadProcessId(self.hwnd, c.byref(owner))
        if owner.value != self.owned_pid:
            raise Blocked("work-area placement lost the exact owned GUI")
        info = MonitorInfo(size=c.sizeof(MonitorInfo))
        monitor = self.u.MonitorFromWindow(self.hwnd, 2)
        if not monitor or not self.u.GetMonitorInfoW(monitor, c.byref(info)):
            raise Blocked("owned monitor work area unavailable")
        width = min(1100, info.work.right - info.work.left - 16)
        height = min(800, info.work.bottom - info.work.top - 16)
        if width < 480 or height < 360:
            raise Blocked("hosted work area too small for native controls")
        if not self.u.SetWindowPos(self.hwnd, None, info.work.left + 8, info.work.top + 8,
                                   width, height, 0x14):  # NOZORDER | NOACTIVATE
            raise Blocked("owned work-area placement failed")
        actual = w.RECT()
        if (not self.u.GetWindowRect(self.hwnd, c.byref(actual))
                or actual.left < info.work.left or actual.top < info.work.top
                or actual.right > info.work.right or actual.bottom > info.work.bottom):
            raise Blocked("owned native bounds exceed the monitor work area")
        self.window_placement = {"work_area": [info.work.left, info.work.top, info.work.right, info.work.bottom],
                                 "outer_bounds": [actual.left, actual.top, actual.right, actual.bottom]}

    def owned_hit(self, hwnd):
        if not hwnd or self.u.GetAncestor(hwnd, 2) != self.hwnd:
            return False
        owner = w.DWORD()
        self.u.GetWindowThreadProcessId(hwnd, c.byref(owner))
        if owner.value == self.owned_pid:
            return True
        # WebView2 can host its actual child HWND in a renderer process. Both
        # the native window ancestry and our private containment must match.
        if not self.u.IsChild(self.hwnd, hwnd) or not self.owned_job:
            return False
        handle = self.k.OpenProcess(0x1000, False, owner.value)
        if not handle:
            return False
        try:
            member = w.BOOL()
            return bool(self.k.IsProcessInJob(handle, self.owned_job, c.byref(member)) and member.value)
        finally:
            self.k.CloseHandle(handle)

    def window_fact(self, hwnd):
        """Finite hosted-court metadata; never titles, command lines or paths."""
        if not hwnd:
            return {"present": False}
        owner = w.DWORD()
        self.u.GetWindowThreadProcessId(hwnd, c.byref(owner))
        expected = w.DWORD()
        self.u.GetWindowThreadProcessId(self.hwnd, c.byref(expected))
        name = c.create_unicode_buffer(128)
        self.u.GetClassNameW(hwnd, name, len(name))
        rect = w.RECT()
        observed = bool(self.u.GetWindowRect(hwnd, c.byref(rect)))
        process_name = None
        in_owned_job = None
        package = {"status": "unavailable"}
        handle = self.k.OpenProcess(0x1000, False, owner.value)
        if handle:
            try:
                member = w.BOOL()
                if self.owned_job and self.k.IsProcessInJob(handle, self.owned_job, c.byref(member)):
                    in_owned_job = bool(member.value)
                family = c.create_unicode_buffer(512)
                family_size = w.DWORD(len(family))
                package_code = self.k.GetPackageFamilyName(handle, c.byref(family_size), family)
                package = ({"status": "observed", "family": family.value} if package_code == 0
                           else {"status": "not-packaged" if package_code == 15700 else "unavailable",
                                 "error_code": package_code})
                image = c.create_unicode_buffer(32768)
                length = w.DWORD(len(image))
                if self.k.QueryFullProcessImageNameW(handle, 0, image, c.byref(length)):
                    process_name = Path(image.value).name
            finally:
                self.k.CloseHandle(handle)
        return {"present": True, "same_process_as_gui": owner.value == expected.value,
                "owned_root": self.u.GetAncestor(hwnd, 2) == self.hwnd,
                "in_owned_job": in_owned_job,
                "package": package,
                "class": name.value, "process_basename": process_name,
                "bounds": [rect.left, rect.top, rect.right, rect.bottom] if observed else None}

    def record_focus(self, stage, hit=None, point=None):
        if len(self.focus_observations) >= 16:
            return
        self.focus_observations.append({
            "stage": stage, "foreground": self.window_fact(self.u.GetForegroundWindow()),
            "owned_topmost_presentation": self.presentation_topmost,
            "owned_gui": self.window_fact(self.hwnd),
            "hit": self.window_fact(hit),
            "point": [point.x, point.y] if point is not None else None,
        })

    def foreground(self):
        if self.u.GetForegroundWindow() != self.hwnd:
            self.record_focus("input-refused-foreign-foreground")
            raise Blocked("owned GUI lost foreground; input stopped")

    def send(self, event):
        self.foreground()
        if self.u.SendInput(1, c.byref(event), c.sizeof(Input)) != 1:
            raise Blocked("SendInput rejected; possible desktop/UIPI restriction")

    def key(self, vk, up=False, cleanup=False):
        event = Input(kind=1, keyboard=KeyboardInput(vk=vk, flags=2 if up else 0))
        if cleanup:
            self.u.SendInput(1, c.byref(event), c.sizeof(Input))
        else:
            self.send(event)
        self.pressed.discard(vk) if up else self.pressed.add(vk)

    def chord(self, *keys):
        if 0x11 in keys and any(key in keys for key in (0x43, 0x58)):
            # Copy/cut can mutate the clipboard via the GUI. Mark ownership
            # before injection so a partial SendInput failure still restores it.
            self.clipboard_dirty = True
            self.clipboard_diagnostics["changed"] = True
        try:
            for key in keys:
                self.key(key)
        finally:
            for key in reversed(keys):
                if key in self.pressed:
                    self.key(key, up=True, cleanup=True)
        time.sleep(0.06)

    def click(self, rect):
        self.foreground()
        assert rect["width"] > 0 and rect["height"] > 0, "empty published hit target"
        point = w.POINT(round(rect["x"] + rect["width"] / 2),
                        round(rect["y"] + rect["height"] / 2))
        assert self.u.ClientToScreen(self.hwnd, c.byref(point)), "ClientToScreen failed"
        assert self.u.SetCursorPos(point.x, point.y), "SetCursorPos failed"
        hit = self.u.WindowFromPoint(point)
        if not self.owned_hit(hit):
            self.record_focus("input-refused-obscured-hit-target", hit, point)
            raise Blocked("owned input point is obscured; no click sent")
        self.send(Input(kind=0, mouse=MouseInput(flags=2)))
        try:
            self.send(Input(kind=0, mouse=MouseInput(flags=4)))
        finally:
            release = Input(kind=0, mouse=MouseInput(flags=4))
            self.u.SendInput(1, c.byref(release), c.sizeof(Input))
        time.sleep(0.08)

    def message(self, hwnd, msg, wp=0, lp=0):
        result = c.c_size_t()
        if not self.u.SendMessageTimeoutW(hwnd, msg, wp, lp, 2, 1000, c.byref(result)):
            raise Blocked("native IME message timed out or target unavailable")

    def ime(self, enabled):
        self.foreground()
        if enabled:
            allocated = os.environ.get('MINICON_COURT_ALLOCATED_DESKTOP')
            if allocated:
                from tsf_profile import AllocatedTsfProfile, TsfUnavailable
                try:
                    if self.tsf_profile is None:
                        self.tsf_profile = AllocatedTsfProfile(self.u, self.k, allocated)
                except TsfUnavailable as error:
                    raise Blocked(str(error)) from error
            import winreg
            # Microsoft documents this exact Pinyin TIP/profile pair. Merely
            # having a Chinese language name is not an installation oracle.
            tip = (r"SOFTWARE\Microsoft\CTF\TIP\{81D4E9C9-1D3B-41BC-9E6C-4B40BF79E35E}"
                   r"\LanguageProfile\0x00000804\{FA550B04-5AD7-411F-A5AC-CA038EC515D7}")
            try:
                with winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, tip,
                                    0, winreg.KEY_READ | winreg.KEY_WOW64_64KEY):
                    pass
            except FileNotFoundError as error:
                raise Blocked("Microsoft Pinyin profile is not installed") from error
            count = self.u.GetKeyboardLayoutList(0, None)
            if not 0 < count <= 128:
                raise Blocked("native keyboard layout count unavailable or oversized")
            layouts = (c.c_void_p * count)()
            actual = self.u.GetKeyboardLayoutList(count, layouts)
            if not 0 < actual <= count:
                raise Blocked("native keyboard layout list unavailable")
            chinese = [layout for layout in layouts[:actual] if layout and layout & 0xFFFF == 0x804]
            if not chinese:
                raise Blocked("Pinyin/Simplified Chinese layout is not already loaded")
            self.message(self.hwnd, 0x50, 0, chinese[0])
            if self.tsf_profile is not None:
                try:
                    self.tsf_profile.select_pinyin(chinese[0])
                except TsfUnavailable as error:
                    raise Blocked(str(error)) from error
        ime_window = self.i.ImmGetDefaultIMEWnd(self.hwnd)
        if not ime_window:
            raise Blocked("owned window has no native IME window")
        self.message(ime_window, 0x283, 6, int(enabled))
        if enabled:
            self.message(ime_window, 0x283, 2, 1)
        time.sleep(0.12)

    def clipboard_text(self, text=None):
        self.foreground()
        end = time.monotonic() + 1
        while not self.u.OpenClipboard(self.hwnd):
            if time.monotonic() >= end:
                raise Blocked("clipboard is held by another application")
            time.sleep(0.02)
        try:
            if text is None:
                handle = self.u.GetClipboardData(13)
                if not handle:
                    return None
                pointer = self.k.GlobalLock(handle)
                if not pointer:
                    raise Blocked("clipboard read unavailable")
                try:
                    size = self.k.GlobalSize(handle)
                    if not 0 < size <= ClipboardSnapshot.FORMAT_LIMIT or size % 2:
                        raise Blocked("clipboard text size unavailable or oversized")
                    return c.string_at(pointer, size).decode("utf-16-le").split("\0", 1)[0]
                finally:
                    self.k.GlobalUnlock(handle)
            encoded = (text + "\0").encode("utf-16-le")
            handle = self.k.GlobalAlloc(2, len(encoded))
            if not handle:
                raise Blocked("clipboard allocation unavailable")
            try:
                pointer = self.k.GlobalLock(handle)
                if not pointer:
                    raise Blocked("clipboard allocation lock unavailable")
                c.memmove(pointer, encoded, len(encoded))
                self.k.GlobalUnlock(handle)
                self.clipboard_dirty = True
                self.clipboard_diagnostics["changed"] = True
                if not self.u.EmptyClipboard() or not self.u.SetClipboardData(13, handle):
                    raise Blocked("clipboard seed unavailable")
                handle = None  # clipboard now owns the allocation
            finally:
                if handle:
                    self.k.GlobalFree(handle)
        finally:
            self.u.CloseClipboard()

    def capture(self, path):
        self.foreground()
        rect = w.RECT()
        assert self.u.GetClientRect(self.hwnd, c.byref(rect)), "owned client bounds unavailable"
        width, height = rect.right, rect.bottom
        if not 1 <= width <= 4096 or not 1 <= height <= 4096:
            raise Blocked("owned client exceeds bounded screenshot size")
        source = self.u.GetDC(self.hwnd)  # never GetDC(NULL) or desktop grab
        memory = self.g.CreateCompatibleDC(source)
        bitmap = self.g.CreateCompatibleBitmap(source, width, height)
        previous = self.g.SelectObject(memory, bitmap) if bitmap and memory else None
        try:
            if not all((source, memory, bitmap, previous)):
                raise Blocked("owned-window GDI capture unavailable")
            if not self.g.BitBlt(memory, 0, 0, width, height, source, 0, 0, 0x00CC0020):
                raise Blocked("owned-window BitBlt failed")
            self.g.SelectObject(memory, previous)
            previous = None
            # 40-byte top-down BITMAPINFOHEADER and one RGBQUAD. Force opaque RGB.
            header = c.create_string_buffer(struct.pack("<IiiHHIIiiII", 40, width, -height,
                                                        1, 32, 0, 0, 0, 0, 0, 0) + b"\0" * 4)
            pixels = c.create_string_buffer(width * height * 4)
            if self.g.GetDIBits(memory, bitmap, 0, height, pixels, header, 0) != height:
                raise Blocked("owned-window GetDIBits failed")
            raw = pixels.raw
            rgb = bytearray(width * height * 3)
            rgb[0::3], rgb[1::3], rgb[2::3] = raw[2::4], raw[1::4], raw[0::4]
            if len(set(rgb)) < 3:
                raise Blocked("owned capture is blank")
            def chunk(kind, data):
                return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
            rows = b"".join(b"\0" + rgb[y * width * 3:(y + 1) * width * 3] for y in range(height))
            path.write_bytes(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
                             + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))
            return {"file": path.name, "width": width, "height": height,
                    "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
                    "mechanism": "owned-HWND-client-DC/BitBlt/GetDIBits", "pixel_review": "PENDING"}
        finally:
            if previous:
                self.g.SelectObject(memory, previous)
            if bitmap:
                self.g.DeleteObject(bitmap)
            if memory:
                self.g.DeleteDC(memory)
            if source:
                self.u.ReleaseDC(self.hwnd, source)

    def restore(self):
        for key in list(self.pressed):
            self.key(key, up=True, cleanup=True)
        if self.hwnd and self.original_layout:
            try:
                self.message(self.hwnd, 0x50, 0, self.original_layout)
            except Blocked:
                pass
        try:
            if not self.clipboard_dirty:
                # A setup failure before clipboard writes needs no Set/Flush.
                # Restoring an untouched clipboard can itself erase/replace data.
                self.clipboard_diagnostics["restoration"] = "not-needed-unchanged"
                return True
            try:
                self.clipboard_snapshot.restore(self.hwnd)
            except ClipboardError as error:
                self.clipboard_diagnostics.update(self.clipboard_snapshot.diagnostics)
                self.clipboard_diagnostics.update(restoration="deep-copy-restore-failed", reason=str(error))
                return False
            self.clipboard_diagnostics.update(self.clipboard_snapshot.diagnostics)
            return True
        finally:
            if self.tsf_profile is not None:
                self.tsf_profile_restored = self.tsf_profile.close()
            if self.presentation_topmost:
                if self.u.IsWindow(self.hwnd) and self.owned_hit(self.hwnd):
                    self.presentation_restored = bool(self.u.SetWindowPos(
                        self.hwnd, w.HWND(-2), 0, 0, 0, 0, 0x13))
                else:
                    self.presentation_restored = not self.u.IsWindow(self.hwnd)
            self.u.SetCursorPos(self.pointer.x, self.pointer.y)
            if self.original_foreground:
                self.u.SetForegroundWindow(self.original_foreground)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--source-sha", required=True, help="exact artifact source from its receipt; not inferred from this checkout")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    if any(output.iterdir()):
        parser.error("--output must be a fresh empty directory")
    receipt = {"status": "BLOCKED", "source_sha": args.source_sha,
               "source_provenance": "caller supplied; compare with artifact manifest",
               "input_path": "Win32 SendInput physical VK + native IME; no send-ui injection",
               "steps": [], "screenshots": [], "scope": "Windows host controls; no provider authorization claim",
               "runner_arch": os.environ.get("RUNNER_ARCH"),
               "python_bits": c.sizeof(c.c_void_p) * 8,
               "script_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
               "native_scale_qualified": False, "production_artifact_qualification": False}
    native = child = server = worker = None
    scratch = tempfile.TemporaryDirectory(prefix="minicon-native-controls-")
    deadline = time.monotonic() + 240
    endpoint = rf"pipe:\\.\pipe\minicon-native-controls-{os.getpid()}-{secrets.token_hex(12)}"
    binary = args.binary.resolve(strict=True)
    receipt["binary_sha256"] = hashlib.sha256(binary.read_bytes()).hexdigest()
    receipt["binary"] = binary.name
    phase = "setup"

    def save():
        (output / "receipt.json").write_text(json.dumps(receipt, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

    def cli(*arguments):
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise Blocked("total 240-second court deadline exhausted")
        result = subprocess.run([str(binary), "cli", "--control", endpoint, *arguments],
                                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                timeout=min(5, remaining), encoding="utf-8", errors="strict")
        if result.returncode:
            raise RuntimeError("public CLI failed: " + arguments[0])
        return json.loads(result.stdout)

    def snapshot():
        return cli("ui-snapshot")

    def geo():
        return snapshot()["geometry"]

    def wait(predicate, label, seconds=4, unavailable=False):
        end = min(deadline, time.monotonic() + seconds)
        while time.monotonic() < end:
            if child is not None and child.poll() is not None:
                raise AssertionError("owned GUI exited during " + label)
            value = predicate()
            if value:
                return value
            time.sleep(0.08)
        raise (Blocked if unavailable else AssertionError)("deadline: " + label)

    def step(name, **fields):
        receipt["steps"].append({"name": name, "status": "passed",
                                 "binary_sha256": receipt["binary_sha256"],
                                 "source_sha": args.source_sha,
                                 "scale_permille": geo()["scale_permille"],
                                 "input_path": "Win32 SendInput/native IME", **fields})
        save()

    def observe(name):
        state = snapshot()
        receipt["scale_permille"] = state["geometry"]["scale_permille"]
        (output / (name + ".json")).write_text(json.dumps(state, ensure_ascii=False, indent=2), encoding="utf-8")
        receipt["screenshots"].append(native.capture(output / (name + ".png")))
        save()
        return state

    def browser():
        return next(row["browser"] for row in cli("list-tabs")["tabs"] if row["kind"] == "browser")

    def click(name):
        native.click(geo()[name])

    def editor(name):
        return geo()[name]

    def paste(text):
        native.clipboard_text(text)
        native.chord(0x11, 0x56)

    def edit_court(name, target):
        native.click(geo()[target])
        wait(lambda: editor(name)["focused"], name + " focus")
        native.ime(False)
        native.chord(0x11, 0x41)
        paste("nativefield")
        wait(lambda: editor(name)["text"] == "nativefield", name + " native paste")
        native.chord(0x11, 0x41)
        wait(lambda: editor(name)["selection"] == [0, 11], name + " select all")
        native.chord(0x11, 0x43)
        assert native.clipboard_text() == "nativefield", "native copy produced wrong clipboard value"
        native.chord(0x11, 0x58)
        wait(lambda: editor(name)["text"] == "", name + " cut")
        assert native.clipboard_text() == "nativefield", "native cut produced wrong clipboard value"
        native.chord(0x11, 0x56)
        wait(lambda: editor(name)["text"] == "nativefield", name + " paste cut text")
        native.chord(0x11, 0x41)
        native.chord(0x08)
        wait(lambda: editor(name)["text"] == "", name + " clear")
        before = browser()
        options = geo()["web_proxy_options_count"]
        native.ime(True)
        def chinese_mode():
            state = snapshot()["ime_status"]
            return state if state["known"] and state["available"] and state["open"] and state["native_mode"] else None
        # IMM requests are not activation evidence for a modern TSF profile.
        # Give the public mode observation time to reflect the request before
        # using the documented physical mode switch exactly once.
        mode_deadline = min(deadline, time.monotonic() + 1)
        activated = chinese_mode()
        while not activated and time.monotonic() < mode_deadline:
            time.sleep(0.04)
            activated = chinese_mode()
        method = "IMM-request-with-public-readback"
        if not activated:
            native.chord(0x10)  # Microsoft Pinyin Chinese/English mode switch.
            method = "one-owned-physical-Shift-with-public-readback"
            activated = wait(chinese_mode, name + " Chinese IME mode activation", 2, unavailable=True)
        receipt.setdefault("ime_mode_activations", []).append({
            "editor": name, "method": method,
            "known": activated["known"], "available": activated["available"],
            "open": activated["open"], "native_mode": activated["native_mode"],
        })
        save()
        for key in "nihao":
            native.chord(ord(key.upper()))
        def complete_preedit():
            value = editor(name)
            normalized = value["preedit"].lower().replace("'", "").replace(" ", "")
            return value if "nihao" in normalized else None
        preedit = wait(complete_preedit, name + " complete real Pinyin preedit", unavailable=True)
        assert "nihao" in preedit["preedit"].lower().replace("'", "").replace(" ", ""), "unexpected native Pinyin preedit"
        assert preedit["text"] == "", "preedit changed committed editor text"
        observe(name + "-preedit")
        native.chord(0x20)
        wait(lambda: editor(name)["preedit"] == "" and editor(name)["text"] == "你好",
             name + " Space candidate commit")
        assert editor(name)["focused"], "IME confirmation unexpectedly blurred/submitted editor"
        after = browser()
        assert after["url"] == before["url"] and after["load"] == before["load"], "IME confirmation navigated/applied proxy"
        assert geo()["web_proxy_options_count"] == options, "IME confirmation added candidate"
        observe(name + "-committed")
        step(name, native_preedit=True, space_commit=True, separate_submit_preserved=True,
             selection_copy_cut_paste=True)
        native.ime(False)

    def harness():
        return geo()["harness_settings"]

    def harness_cell(row_id, column=0):
        for _ in range(40):
            state = harness()
            if state["scroll"] == 0:
                break
            native.click(state["previous"])
        for _ in range(40):
            state = harness()
            for row in state["rows"]:
                if row["id"] == row_id:
                    return row["cells"][column]
            native.click(state["next"])
        raise AssertionError("published Harness row unavailable: " + row_id)

    try:
        if sys.platform != "win32":
            raise Blocked("this physical court requires Windows interactive desktop")
        if len(args.source_sha) != 40 or any(ch not in "0123456789abcdef" for ch in args.source_sha):
            raise Blocked("--source-sha must be the exact 40-character lowercase artifact source SHA")
        native = Native()
        echoes = []
        class Page(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_GET(self):
                parsed = urllib.parse.urlsplit(self.path)
                if parsed.path == "/typed":
                    echoes.append(urllib.parse.parse_qs(parsed.query).get("value", [""])[0])
                    body = b"ok"
                else:
                    body = (b"<!doctype html><title>MiniCon native controls</title>"
                            b"<body style='margin:0;background:#eef3fb'><input autofocus "
                            b"style='position:absolute;left:20px;top:20px;width:280px;height:70px;font-size:24px' "
                            b"oninput=\"fetch('/typed?value='+encodeURIComponent(this.value))\">"
                            b"<h1 style='position:absolute;top:120px'>Owned native page</h1></body>")
                self.send_response(200)
                self.send_header("Content-Type", "text/html; charset=utf-8")
                self.send_header("Content-Length", str(len(body)))
                self.send_header("Connection", "close")
                self.end_headers()
                self.wfile.write(body)
        class BoundedServer(http.server.HTTPServer):
            def get_request(self):
                connection, address = super().get_request()
                connection.settimeout(1)
                return connection, address
        server = BoundedServer(("127.0.0.1", 0), Page)
        server.timeout = 0.1
        stop_server = threading.Event()
        def serve():
            while not stop_server.is_set():
                server.handle_request()
        worker = threading.Thread(target=serve, name="owned-native-page", daemon=False)
        worker.start()
        env = os.environ.copy()
        for key in list(env):
            if key.upper().startswith(("MINICON_", "WEBVIEW2_", "DEEPSEEK_", "OPENCODE_", "OPENAI_", "ANTHROPIC_")):
                env.pop(key)
        env["MINICON_CONFIG_DIR"] = str(Path(scratch.name) / "config")
        env["MINICON_BROWSER_PROFILE_ROOT"] = str(Path(scratch.name) / "profile")
        # Native acceptance needs normal activation, matching the existing
        # spawn_with_activation(true) Pinyin court. No AttachThreadInput bypass.
        child = OwnedProcess(binary, ["--control", endpoint,
                                      "-e", "cmd.exe", "/d", "/k"], env)
        def ready():
            try:
                return cli("list-tabs")
            except (RuntimeError, subprocess.TimeoutExpired, json.JSONDecodeError):
                return None
        wait(ready, "owned control endpoint ready", 20, unavailable=True)
        native.attach(child.pid, child.job)
        receipt["owned_pid"] = child.pid
        receipt["native_activation"] = native.activation
        receipt["native_window_placement"] = native.window_placement
        wait(lambda: geo()["scale_permille"] > 0, "published geometry")
        click("new_root")
        wait(lambda: geo()["new_tab_menu_open"], "New menu")
        click("new_browser_action")
        wait(lambda: any(row["kind"] == "browser" for row in cli("list-tabs")["tabs"]), "Browser created")
        native.chord(0x11, 0x4C)
        wait(lambda: geo()["browser_address_editor"]["focused"], "address ready")
        native.ime(False)
        native.chord(0x11, 0x41)
        origin = f"http://127.0.0.1:{server.server_port}/"
        paste(origin)
        native.chord(0x0D)
        wait(lambda: browser()["load"] == "ready" and browser()["url"] == origin,
             "real loopback native page loaded", 25, unavailable=True)
        phase = "native-page-keyboard"
        page = geo()["browser_page"]
        native.click({"x": page["x"] + 20, "y": page["y"] + 20, "width": 200, "height": 60})
        info = GuiThreadInfo(size=c.sizeof(GuiThreadInfo))
        assert native.u.GetGUIThreadInfo(0, c.byref(info)), "foreground GUI thread info unavailable"
        assert info.focus != native.hwnd and native.u.IsChild(native.hwnd, info.focus), "native page did not own OS keyboard focus"
        native.chord(0x51)
        wait(lambda: "q" in echoes, "physical key reached native page input", unavailable=True)
        native.chord(0x11, 0x4C)
        wait(lambda: editor("browser_address_editor")["focused"], "native page Ctrl+L reaches host address")
        native.chord(0x09)
        wait(lambda: editor("browser_proxy_editor")["focused"], "Tab address to proxy")
        native.chord(0x09)
        wait(lambda: geo()["browser_chrome_trigger"] == "proxy", "Tab proxy to menu trigger")
        native.chord(0x20)
        wait(lambda: geo()["browser_input_owner"] == "proxy_menu", "Space opens proxy menu")
        native.chord(0x1B)
        wait(lambda: geo()["browser_chrome_trigger"] == "proxy", "Escape restores trigger")
        native.chord(0x09)
        wait(lambda: geo()["harness_workspace"]["header_focus"] == "robot", "Tab reaches Robot header")
        native.chord(0x09)
        wait(lambda: geo()["harness_workspace"]["header_focus"] == "settings", "Tab reaches Settings header")
        native.chord(0x09)
        wait(lambda: geo()["browser_chrome_trigger"] == "new", "Tab cycles to New trigger")
        native.chord(0x0D)
        wait(lambda: geo()["new_tab_menu_open"], "Enter opens New menu")
        observe("native-keyboard-new-menu")
        native.chord(0x1B)
        assert len(cli("list-tabs")["tabs"]) == 2, "opening menu activated an action"
        native.chord(0x10, 0x09)
        wait(lambda: geo()["harness_workspace"]["header_focus"] == "settings", "Shift+Tab reaches Settings header")
        native.chord(0x10, 0x09)
        wait(lambda: geo()["harness_workspace"]["header_focus"] == "robot", "Shift+Tab reaches Robot header")
        native.chord(0x10, 0x09)
        wait(lambda: geo()["browser_chrome_trigger"] == "proxy", "Shift+Tab returns to proxy trigger")
        step(phase, native_page_echo=True, ctrl_l=True, tab_menu=True,
             robot_settings_join_forward_reverse=True)
        phase = "three-editor-native-ime"
        edit_court("browser_address_editor", "browser_address")
        native.chord(0x1B)
        edit_court("browser_proxy_editor", "browser_proxy")
        native.chord(0x1B)
        click("settings")
        wait(lambda: snapshot()["settings_open"], "Settings opens")
        click("settings_web_route")
        wait(lambda: geo()["settings_section"] == "web", "Web route")
        edit_court("web_proxy_candidate_editor", "web_proxy_input")
        phase = "harness-routes-fields-and-redaction"
        click("settings_harness_route")
        wait(lambda: geo()["settings_section"] == "harness" and not harness()["busy"], "Harness initialized")
        initial_port = harness()["port"]
        native.click(harness_cell("port"))
        wait(lambda: harness()["port_editor"]["focused"], "Harness port focus")
        native.chord(0x11, 0x41)
        paste("30127")
        wait(lambda: harness()["port_editor"]["text"] == "30127", "Harness port native paste")
        assert harness()["port"] == initial_port, "port draft saved before Submit"
        native.click(harness_cell("model_input"))
        wait(lambda: harness()["model_editor"]["focused"], "Harness model focus")
        paste("native-model-draft")
        wait(lambda: harness()["model_editor"]["text"] == "native-model-draft", "Harness model native paste")
        assert harness()["model"] == "", "model draft applied before Submit"
        observe("harness-fields")
        for section in ("console", "web", "harness"):
            click("settings_" + section + "_route")
            wait(lambda: geo()["settings_section"] == section, section + " route")
            observe("route-" + section)
        assert harness()["port_editor"]["text"] == "30127", "route switch lost port draft"
        assert harness()["model_editor"]["text"] == "native-model-draft", "route switch lost model draft"
        native.click(harness_cell("key_actions", 0))
        wait(lambda: harness()["local_key_exists"], "explicit local key creation")
        key = (Path(scratch.name) / "config" / "harness" / "client-key").read_text(encoding="utf-8").strip()
        assert key, "local key storage missing"
        state = snapshot()
        assert key not in json.dumps(state), "snapshot leaked local client key"
        public = state["geometry"]["harness_settings"]
        assert not ({"access_token", "refresh_token", "user_code", "verification_url"} & public.keys()), "secret-bearing snapshot field exposed"
        metadata = public["client_key"]
        assert set(metadata) == {"profile", "applied", "exists", "state",
                                 "durability_unconfirmed", "pending", "feedback"}, "unexpected client-key metadata field"
        assert metadata["profile"] == "default" and metadata["exists"] is True, "generated key metadata does not describe selected profile"
        assert metadata["pending"] is None, "key operation is not terminal"
        observe("harness-key-redacted")
        for path in output.glob("*.json"):
            assert key not in path.read_text(encoding="utf-8"), "receipt/snapshot leaked local key"
        native.click(harness_cell("key_actions", 2))
        wait(lambda: not harness()["local_key_exists"], "explicit local key revocation")
        step(phase, routes=["console", "web", "harness"], port_model_native_input=True,
             manual_binding_preserved=True, snapshot_local_key_redacted=True,
             provider_auth="NOT_ATTEMPTED", pixel_review="PENDING")
        receipt["status"] = "passed"
    except Blocked as error:
        receipt.update(status="BLOCKED", phase=phase, reason=str(error))
    except Exception as error:
        # Exceptions are fixed labels/OS errors; never dump credentials or full child output.
        receipt.update(status="failed", phase=phase, reason=f"{type(error).__name__}: {error}")
    finally:
        if receipt["status"] != "passed" and child is not None and child.poll() is None:
            # Synthetic isolated GUI only. Preserve the actual failed state
            # before cleanup; never change a predicate from an absent snapshot.
            try:
                failed_state = snapshot()
                (output / "failure.json").write_text(json.dumps(failed_state, ensure_ascii=False, indent=2), encoding="utf-8")
                receipt["failure_snapshot"] = "failure.json"
                if native is not None:
                    receipt["owned_window_foreground_at_failure"] = native.u.GetForegroundWindow() == native.hwnd
                    foreground_pid = w.DWORD()
                    native.u.GetWindowThreadProcessId(native.u.GetForegroundWindow(), c.byref(foreground_pid))
                    receipt["foreground_is_owned_gui_at_failure"] = foreground_pid.value == child.pid
                    if receipt["owned_window_foreground_at_failure"]:
                        receipt["screenshots"].append(native.capture(output / "failure.png"))
            except (OSError, RuntimeError, AssertionError, subprocess.SubprocessError, json.JSONDecodeError):
                receipt["failure_snapshot_unavailable"] = True
        cleanup = {}
        if native is not None:
            receipt["native_focus_observations"] = native.focus_observations
            receipt["owned_topmost_for_presentation"] = native.presentation_topmost
            if native.tsf_profile is not None:
                receipt["tsf_profile_observations"] = native.tsf_profile.records
            try:
                cleanup["clipboard_restored"] = native.restore()
            except (OSError, Blocked):
                cleanup["clipboard_restored"] = False
            cleanup["clipboard"] = native.clipboard_diagnostics
            cleanup["owned_presentation_restored"] = native.presentation_restored
            cleanup["tsf_profile_restored"] = native.tsf_profile_restored
        if child is not None:
            if child.poll() is None:
                try:
                    cli("close-window")
                    child.wait(timeout=8)
                    cleanup["gui_closed"] = True
                except (OSError, RuntimeError, subprocess.SubprocessError):
                    child.k.TerminateJobObject(child.job, 1)
                    cleanup["forced_owned_tree_cleanup"] = True
                    try:
                        child.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        cleanup["owned_gui_reaped"] = False
            cleanup["host_exit_code"] = child.poll()
            child.close()  # final Job close also kills any surviving owned descendants
        if server is not None:
            stop_server.set()
            worker.join(timeout=2)
            server.server_close()
            cleanup["page_thread_joined"] = not worker.is_alive()
        try:
            scratch.cleanup()
            cleanup["isolated_config_removed"] = True
        except OSError:
            cleanup["isolated_config_removed"] = False
        receipt["cleanup"] = cleanup
        if (any(value is False for value in cleanup.values()) or cleanup.get("forced_owned_tree_cleanup")
                or (child is not None and cleanup.get("host_exit_code") != 0)):
            if receipt["status"] == "passed":
                receipt["status"] = "BLOCKED"
            receipt["cleanup_reason"] = "clean restoration/graceful-owned-window shutdown not proven"
        save()
    print(json.dumps({"status": receipt["status"], "receipt": str(output / "receipt.json")}, ensure_ascii=False))
    return {"passed": 0, "BLOCKED": 2, "failed": 1}[receipt["status"]]


if __name__ == "__main__":
    raise SystemExit(main())
