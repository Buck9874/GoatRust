"""Find UE3 native function tables in GoatGame.exe and name their functions.

UE3 registers natives as { "execFoo", &UClass::execFoo } pairs (FNativeFunctionLookup), usually
with the class's tables built from MAP_NATIVE.  Ghidra does not define most of those ANSI names
as strings, so this scans the raw image: every "exec<Name>\\0" in a data section whose address is
followed by (or preceded by) a pointer into .text names that function.

Writes out/decomp/native/exec_names.tsv (address, name, table address) and, with --apply,
rewrites FUN_<addr> -> <name> in the decompiled .c files.

usage: python -I tools/name_natives.py <exe> [--apply]
"""
import os
import re
import struct
import sys

root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
exe = sys.argv[1] if len(sys.argv) > 1 and not sys.argv[1].startswith('--') else os.path.join(root, 'out', 'rea', 'GoatGame-native.exe')
apply = '--apply' in sys.argv
data = open(exe, 'rb').read()

pe = struct.unpack_from('<I', data, 0x3c)[0]
nsec = struct.unpack_from('<H', data, pe + 6)[0]
opt_size = struct.unpack_from('<H', data, pe + 20)[0]
image_base = struct.unpack_from('<I', data, pe + 24 + 28)[0]
sections = []
for i in range(nsec):
    off = pe + 24 + opt_size + i * 40
    name = data[off:off + 8].rstrip(b'\0').decode('latin1')
    vsize, va, rsize, raw = struct.unpack_from('<IIII', data, off + 8)
    sections.append((name, image_base + va, vsize, raw, rsize))


def sec(name):
    return next(s for s in sections if s[0] == name)


text = sec('.text')


def in_text(v):
    return text[1] <= v < text[1] + text[2]


# ANSI exec names anywhere in the read-only/initialised data
names = {}
for sname in ('.rdata', '.data'):
    _, va, vsize, raw, rsize = sec(sname)
    blob = data[raw:raw + rsize]
    # "AActorexecMoveSmooth": the class's C++ name, then the native's name
    for m in re.finditer(rb'([AUF][A-Za-z0-9_]{1,60}?)exec([A-Za-z0-9_]{1,80})\x00', blob):
        if m.start() > 0 and blob[m.start() - 1] != 0:
            continue  # part of a longer string
        names[va + m.start()] = m.group(1).decode() + '_exec' + m.group(2).decode()
print(f'{len(names)} exec name strings')

found = {}
for sname in ('.rdata', '.data'):
    _, va, vsize, raw, rsize = sec(sname)
    blob = data[raw:raw + rsize]
    n = len(blob) // 4
    words = struct.unpack_from(f'<{n}I', blob)
    for i, w in enumerate(words):
        nm = names.get(w)
        if nm is None:
            continue
        for j in (i + 1, i - 1):
            if 0 <= j < n and in_text(words[j]):
                found.setdefault(words[j], (nm, va + i * 4))
                break
print(f'{len(found)} natives named')

out_dir = os.path.join(root, 'out', 'decomp', 'native')
os.makedirs(out_dir, exist_ok=True)
with open(os.path.join(out_dir, 'exec_names.tsv'), 'w', encoding='utf-8') as fh:
    fh.write('address\tname\ttable_entry\n')
    for a in sorted(found):
        fh.write(f'{a:08x}\t{found[a][0]}\t{found[a][1]:08x}\n')

if apply:
    ren = {f'FUN_{a:08x}': nm for a, (nm, _) in found.items()}
    pat = re.compile(r'\bFUN_[0-9a-f]{8}\b')
    changed = 0
    for f in sorted(os.listdir(out_dir)):
        if not (f.endswith('.c') or f == 'functions.tsv'):
            continue
        p = os.path.join(out_dir, f)
        s = open(p, encoding='utf-8').read()
        t = pat.sub(lambda m: ren.get(m.group(), m.group()), s)
        if t != s:
            open(p, 'w', encoding='utf-8').write(t)
            changed += 1
    print(f'renamed in {changed} files')
