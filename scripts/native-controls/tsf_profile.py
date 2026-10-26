"""Exact TSF setup within the hosted allocator's desktop. Owner: PRD 32.

Internal helper only, invoked after Native's hosted guard. ABI and desktop
scope follow the Windows SDK msctf.h and ITfInputProcessorProfileMgr contract.
"""
import ctypes as c
from ctypes import wintypes as w
import uuid


class TsfUnavailable(RuntimeError):
    pass


class Guid(c.Structure):
    _fields_ = [('first', w.DWORD), ('second', w.WORD), ('third', w.WORD),
                ('last', c.c_ubyte * 8)]

    @classmethod
    def parse(cls, value):
        return cls.from_buffer_copy(uuid.UUID(value).bytes_le)


class Profile(c.Structure):
    _fields_ = [('kind', w.DWORD), ('language', w.WORD), ('clsid', Guid),
                ('profile', Guid), ('category', Guid), ('substitute', w.HKL),
                ('caps', w.DWORD), ('layout', w.HKL), ('flags', w.DWORD)]


PROFILES_CLASS = Guid.parse('33c53a50-f456-4884-b049-85fd643ecfed')
MANAGER = Guid.parse('71c6e74c-0f28-11d8-a82a-00065b84435c')
KEYBOARD = Guid.parse('34745c63-b2f0-4784-8b67-5e12c8701a31')
PINYIN = Guid.parse('81d4e9c9-1d3b-41bc-9e6c-4b40bf79e35e')
PINYIN_PROFILE = Guid.parse('fa550b04-5ad7-411f-a5ac-ca038ec515d7')
DESKTOP_FLAGS = 0x20000000 | 0x4  # FORSESSION | DONTCARECURRENTINPUTLANGUAGE


def same_profile(left, right):
    if left.kind != right.kind or left.language != right.language:
        return False
    if left.kind == 1:
        return bytes(left.clsid) == bytes(right.clsid) and bytes(left.profile) == bytes(right.profile)
    return left.kind == 2 and left.layout == right.layout


def pinyin_profile():
    return Profile(kind=1, language=0x804, clsid=PINYIN, profile=PINYIN_PROFILE, category=KEYBOARD)


class AllocatedTsfProfile:
    def __init__(self, user, kernel, expected_desktop):
        # Use only the allocator's calling-thread desktop; the flag alone is
        # not sufficient. No COM/native profile operation precedes readback.
        user.GetThreadDesktop.argtypes, user.GetThreadDesktop.restype = [w.DWORD], w.HANDLE
        kernel.GetCurrentThreadId.argtypes, kernel.GetCurrentThreadId.restype = [], w.DWORD
        user.GetUserObjectInformationW.argtypes = [w.HANDLE, c.c_int, c.c_void_p, w.DWORD, c.POINTER(w.DWORD)]
        user.GetUserObjectInformationW.restype = w.BOOL
        name = c.create_unicode_buffer(512)
        needed = w.DWORD()
        desktop = user.GetThreadDesktop(kernel.GetCurrentThreadId())
        if (not expected_desktop.startswith('minicon-court-')
                or not user.GetUserObjectInformationW(desktop, 2, name, c.sizeof(name), c.byref(needed))
                or name.value != expected_desktop):
            raise TsfUnavailable('exact allocated calling-thread desktop required for TSF setup')
        self.pointer = c.c_void_p()
        self.initialized = False
        self.original = None
        self.changed = False
        self.records = []
        self.user = user
        user.ActivateKeyboardLayout.argtypes, user.ActivateKeyboardLayout.restype = [w.HKL, w.UINT], w.HKL
        self.original_layout = user.GetKeyboardLayout(0)
        if not self.original_layout:
            raise TsfUnavailable('original allocated-thread keyboard layout unavailable')
        self.ole = c.WinDLL('ole32', use_last_error=True)
        self.ole.CoInitializeEx.argtypes, self.ole.CoInitializeEx.restype = [c.c_void_p, w.DWORD], w.LONG
        self.ole.CoCreateInstance.argtypes = [c.POINTER(Guid), c.c_void_p, w.DWORD, c.POINTER(Guid), c.POINTER(c.c_void_p)]
        self.ole.CoCreateInstance.restype = w.LONG
        self.ole.CoUninitialize.argtypes, self.ole.CoUninitialize.restype = [], None
        try:
            hr = self.ole.CoInitializeEx(None, 2)
            self.require_ok(hr, 'COM apartment')
            self.initialized = True
            hr = self.ole.CoCreateInstance(c.byref(PROFILES_CLASS), None, 1, c.byref(MANAGER), c.byref(self.pointer))
            self.require_ok(hr, 'TSF profile manager')
            if not self.pointer:
                raise TsfUnavailable('TSF profile manager returned no interface')
            self.original = self.active()
            if self.original.kind not in (1, 2):
                raise TsfUnavailable('original TSF keyboard profile unavailable')
            self.records.append(self.fact('original', self.original))
        except Exception:
            self.close()
            raise

    @staticmethod
    def require_ok(hr, stage):
        if hr < 0:
            raise TsfUnavailable(f'{stage} unavailable (HRESULT {hr & 0xFFFFFFFF:08x})')

    def method(self, slot, result, types):
        table = c.cast(self.pointer, c.POINTER(c.POINTER(c.c_void_p))).contents
        return c.WINFUNCTYPE(result, c.c_void_p, *types)(table[slot])

    def active(self):
        profile = Profile()
        hr = self.method(10, w.LONG, [c.POINTER(Guid), c.POINTER(Profile)])(
            self.pointer, c.byref(KEYBOARD), c.byref(profile))
        if hr != 0:
            raise TsfUnavailable(f'active TSF keyboard profile unavailable (HRESULT {hr & 0xFFFFFFFF:08x})')
        return profile

    def activate(self, profile):
        hr = self.method(3, w.LONG, [w.DWORD, w.WORD, c.POINTER(Guid), c.POINTER(Guid), w.HKL, w.DWORD])(
            self.pointer, profile.kind, profile.language, c.byref(profile.clsid),
            c.byref(profile.profile), profile.layout if profile.kind == 2 else None, DESKTOP_FLAGS)
        if hr != 0:
            raise TsfUnavailable(f'exact TSF profile activation unavailable (HRESULT {hr & 0xFFFFFFFF:08x})')

    @staticmethod
    def fact(stage, profile):
        return {'stage': stage, 'profile_type': profile.kind, 'language': profile.language,
                'exact_microsoft_pinyin': same_profile(profile, pinyin_profile())}

    def select_pinyin(self, chinese_layout):
        if len(self.records) >= 8:
            raise TsfUnavailable('bounded TSF profile observation capacity exhausted')
        current = self.active()
        wanted = pinyin_profile()
        self.records.append(self.fact('before-selection', current))
        if current.language != wanted.language:
            # The COM caller also needs the requested current input language;
            # a deferred language-mismatch activation cannot prove selection.
            self.changed = True
            if not self.user.ActivateKeyboardLayout(chinese_layout, 0):
                raise TsfUnavailable('allocated-thread Chinese keyboard activation unavailable')
            current = self.active()
        if not same_profile(current, wanted):
            # Mark before dispatch: even a failed activation must restore its
            # exact former profile rather than infer that nothing changed.
            self.changed = True
            self.activate(wanted)
        selected = self.active()
        self.records.append(self.fact('after-selection', selected))
        if not same_profile(selected, wanted):
            raise TsfUnavailable('TSF activation did not select exact Microsoft Pinyin')

    def close(self):
        if not self.pointer:
            if self.initialized:
                self.ole.CoUninitialize()
                self.initialized = False
            return True
        restored = True
        try:
            if self.changed and self.original is not None:
                if not self.user.ActivateKeyboardLayout(self.original_layout, 0):
                    raise TsfUnavailable('allocated-thread original keyboard restoration failed')
                self.activate(self.original)
                restored = same_profile(self.active(), self.original)
        except (TsfUnavailable, OSError):
            restored = False
        finally:
            if self.pointer:
                self.method(2, w.ULONG, [])(self.pointer)
                self.pointer = c.c_void_p()
            if self.initialized:
                self.ole.CoUninitialize()
                self.initialized = False
        return restored
