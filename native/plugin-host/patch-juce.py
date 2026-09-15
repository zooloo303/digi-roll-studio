#!/usr/bin/env python3
"""JUCE 7 compatibility: disable unused native window screenshots on new SDKs.
Does not change editor rendering. Retain this modification with corresponding source.
"""
from pathlib import Path
import sys
p = Path(sys.argv[1]) / 'modules/juce_gui_basics/native/juce_Windowing_mac.mm'
data = p.read_bytes()
if b'// DRS P1:' in data:
    sys.exit(0)
start = data.index(b'static Image createNSWindowSnapshot (NSWindow* nsWindow)')
end = data.index(b'Image createSnapshotOfNativeWindow', start)
replacement = b'''// DRS P1: native screenshots are unused; removed API fails on macOS 27 SDK.
static Image createNSWindowSnapshot (NSWindow*) { return {}; }

'''
if b'// DRS P1:' not in data:
    p.write_bytes(data[:start] + replacement + data[end:])
