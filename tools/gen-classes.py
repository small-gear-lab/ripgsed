#!/usr/bin/env python3
# Regenerates src/regex/classes_table.rs from the C.utf8 locale of the C library:
#   tools/gen-classes.py src/regex/classes_table.rs
import ctypes, ctypes.util, sys
libc=ctypes.CDLL(ctypes.util.find_library('c'))
libc.setlocale.restype=ctypes.c_char_p
assert libc.setlocale(6,b"C.utf8")  # LC_ALL
libc.wctype.restype=ctypes.c_ulong
libc.iswctype.argtypes=[ctypes.c_uint32,ctypes.c_ulong]
names=["alnum","alpha","blank","cntrl","graph","lower","print","punct","space","upper"]
out=[]
for n in names:
    t=libc.wctype(n.encode())
    ranges=[];start=None
    for cp in range(0,0x110000):
        if 0xd800<=cp<0xe000: ok=False if start is None else None
        r = False if 0xd800<=cp<0xe000 else bool(libc.iswctype(cp,t))
        if r and start is None: start=cp
        if not r and start is not None:
            ranges.append((start,cp-1));start=None
    if start is not None: ranges.append((start,0x10ffff))
    out.append((n,ranges))
    print(n,len(ranges),file=sys.stderr)
with open(sys.argv[1],'w') as f:
    f.write("//! Character class tables of glibc's C.utf8 locale, generated from `iswctype` (see\n//! `tools/gen-classes.py`); do not edit by hand.\n\n")
    for n,rs in out:
        f.write(f"pub static {n.upper()}: &[(u32, u32)] = &[\n")
        line="   "
        for a,b in rs:
            item=f" ({a:#x}, {b:#x}),"
            if len(line)+len(item)>100: f.write(line+"\n"); line="   "
            line+=item
        f.write(line+"\n];\n\n")
