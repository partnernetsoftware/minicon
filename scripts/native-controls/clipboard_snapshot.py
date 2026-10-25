"""Fail-closed, in-memory Win32 text clipboard snapshot for the hosted native court.

Only CF_TEXT/CF_OEMTEXT/CF_UNICODETEXT/CF_LOCALE are supported. Unknown formats
block before mutation; this does not implement general OLE/GDI clipboard cloning.
Never print or serialize saved bytes. Owner: PRD 25.02 native acceptance; PRD 32 invocation.
"""
from __future__ import annotations

import ctypes as c
from ctypes import wintypes as w
from contextlib import contextmanager
import time


class ClipboardError(RuntimeError):
    pass


class ClipboardSnapshot:
    FORMATS = frozenset((1, 7, 13, 16))
    FORMAT_LIMIT = 4 * 1024 * 1024
    TOTAL_LIMIT = 8 * 1024 * 1024

    def __init__(self):
        self.u = c.WinDLL("user32", use_last_error=True)
        self.k = c.WinDLL("kernel32", use_last_error=True)
        for dll, name, result, arguments in [
            (self.u, "OpenClipboard", w.BOOL, [w.HWND]),
            (self.u, "CloseClipboard", w.BOOL, []),
            (self.u, "EnumClipboardFormats", w.UINT, [w.UINT]),
            (self.u, "GetClipboardData", w.HANDLE, [w.UINT]),
            (self.u, "EmptyClipboard", w.BOOL, []),
            (self.u, "SetClipboardData", w.HANDLE, [w.UINT, w.HANDLE]),
            (self.k, "GlobalSize", c.c_size_t, [w.HGLOBAL]),
            (self.k, "GlobalLock", c.c_void_p, [w.HGLOBAL]),
            (self.k, "GlobalUnlock", w.BOOL, [w.HGLOBAL]),
            (self.k, "GlobalAlloc", w.HGLOBAL, [w.UINT, c.c_size_t]),
            (self.k, "GlobalFree", w.HGLOBAL, [w.HGLOBAL]),
        ]:
            function = getattr(dll, name)
            function.restype, function.argtypes = result, arguments
        self.data = None
        self.diagnostics = {"mechanism": "bounded-owned-HGLOBAL-deep-copy",
                            "supported_format_ids": sorted(self.FORMATS)}

    def fail(self, operation):
        raise ClipboardError(f"{operation} failed (Win32={c.get_last_error()})")

    @contextmanager
    def opened(self, hwnd=None):
        end = time.monotonic() + 1
        while not self.u.OpenClipboard(hwnd):
            if time.monotonic() >= end:
                self.fail("OpenClipboard")
            time.sleep(0.02)
        try:
            yield
        finally:
            if not self.u.CloseClipboard():
                self.fail("CloseClipboard")

    def formats(self):
        formats = []
        previous = 0
        for _ in range(64):
            c.set_last_error(0)
            current = self.u.EnumClipboardFormats(previous)
            if not current:
                if c.get_last_error():
                    self.fail("EnumClipboardFormats")
                return tuple(formats)
            if current in formats:
                raise ClipboardError("clipboard format enumeration repeated")
            formats.append(current)
            previous = current
        raise ClipboardError("clipboard format enumeration exceeded 64 entries")

    def read_open(self):
        formats = self.formats()
        if not set(formats) <= self.FORMATS:
            # Report only the capability mismatch, never data or format names.
            raise ClipboardError("unsupported clipboard format; original clipboard left untouched")
        captured = []
        total = 0
        for format_id in formats:
            handle = self.u.GetClipboardData(format_id)
            if not handle:
                self.fail("GetClipboardData")
            size = self.k.GlobalSize(handle)
            if not 0 < size <= self.FORMAT_LIMIT or total + size > self.TOTAL_LIMIT:
                raise ClipboardError("clipboard snapshot exceeds 4MiB/format or 8MiB total, or has invalid size")
            pointer = self.k.GlobalLock(handle)
            if not pointer:
                self.fail("GlobalLock")
            try:
                captured.append((format_id, c.string_at(pointer, size)))
            finally:
                self.k.GlobalUnlock(handle)
            total += size
        if self.formats() != formats:
            raise ClipboardError("clipboard format enumeration changed during snapshot")
        return captured

    def capture(self, hwnd=None):
        # Preflight and copy all formats under one clipboard lock. No mutation.
        with self.opened(hwnd):
            captured = self.read_open()
        self.data = captured
        self.diagnostics.update(original_empty=not captured,
                                original_format_ids=[item[0] for item in captured],
                                saved_bytes=sum(len(item[1]) for item in captured))

    def replace_open(self, data):
        if len({item[0] for item in data}) != len(data) or sum(len(item[1]) for item in data) > self.TOTAL_LIMIT:
            raise ClipboardError("replacement repeats formats or exceeds 8MiB total")
        allocated = []
        try:
            # Allocate every independent HGLOBAL before destroying current data.
            for format_id, raw in data:
                if format_id not in self.FORMATS or not 0 < len(raw) <= self.FORMAT_LIMIT:
                    raise ClipboardError("unsupported snapshot replacement format/size")
                handle = self.k.GlobalAlloc(0x42, len(raw))  # MOVEABLE | ZEROINIT
                if not handle:
                    self.fail("GlobalAlloc")
                allocated.append([format_id, handle])
                pointer = self.k.GlobalLock(handle)
                if not pointer:
                    self.fail("GlobalLock allocation")
                try:
                    c.memmove(pointer, raw, len(raw))
                finally:
                    self.k.GlobalUnlock(handle)
            if not self.u.EmptyClipboard():
                self.fail("EmptyClipboard")
            for entry in allocated:
                format_id, handle = entry
                if not self.u.SetClipboardData(format_id, handle):
                    self.fail("SetClipboardData")
                entry[1] = None  # ownership transferred only on success
        finally:
            for _, handle in allocated:
                if handle:
                    self.k.GlobalFree(handle)

    def restore(self, hwnd):
        if self.data is None:
            raise ClipboardError("clipboard snapshot missing")
        if not hwnd:
            raise ClipboardError("valid owned clipboard window required for restoration")
        with self.opened(hwnd):
            self.replace_open(self.data)
        # Verify after closing/reopening, so Windows' synthesized formats are
        # included too. Equality checks byte values internally, never logs them.
        with self.opened(hwnd):
            actual = self.read_open()
        if dict(actual) != dict(self.data):
            expected = dict(self.data)
            observed = dict(actual)
            self.diagnostics["verification"] = {
                "actual_format_ids": sorted(observed),
                "mismatch_format_ids": sorted(format_id for format_id in expected.keys() | observed.keys()
                                              if expected.get(format_id) != observed.get(format_id)),
                "expected_sizes": {str(format_id): len(raw) for format_id, raw in self.data},
                "actual_sizes": {str(format_id): len(raw) for format_id, raw in actual},
            }
            raise ClipboardError("restored clipboard format set or exact bytes differ")
        self.diagnostics["restoration"] = "all-original-formats-exact-bytes-verified"

