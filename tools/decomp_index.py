"""Index the decompiled GoatGame.exe (out/decomp/native) against the shipped UnrealScript.

* natives.tsv: execXxx native -> address, the script classes declaring `native ... function Xxx`
  (with the declaring line) and the file holding its C.
* events.tsv : eventXxx thunks -> address, classes declaring `event Xxx`.
* out/decomp/README.md: what is where.

usage: python -I tools/decomp_index.py [out/decomp]
"""
import collections
import os
import re
import sys

root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
decomp = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else os.path.join(root, 'out', 'decomp'))
native_dir = os.path.join(decomp, 'native')
scripts = os.path.join(root, 'out', 'scripts')

# script declarations: function name -> [(package.class, line)]
native_decl = collections.defaultdict(list)
event_decl = collections.defaultdict(list)
fn_re = re.compile(r'^\s*((?:\w+(?:\([^)]*\))?\s+)*)(function|event)\s+(?:[\w<>]+\s+)?(\w+)\s*\(', re.I)
n_scripts = 0
for pkg in sorted(os.listdir(scripts)):
    d = os.path.join(scripts, pkg)
    if not os.path.isdir(d):
        continue
    for f in sorted(os.listdir(d)):
        if not f.endswith('.uc'):
            continue
        n_scripts += 1
        cls = pkg + '.' + f[:-3]
        with open(os.path.join(d, f), encoding='utf-8', errors='replace') as fh:
            for line in fh:
                m = fn_re.match(line)
                if not m:
                    continue
                mods, kind, name = m.group(1).lower(), m.group(2).lower(), m.group(3)
                if 'native' in mods:
                    native_decl[name].append((cls, line.strip()))
                if kind == 'event':
                    event_decl[name].append((cls, line.strip()))

# functions and the file each one lives in
all_files = sorted(f for f in os.listdir(native_dir) if f.startswith('native_') and f.endswith('.c'))
files = [f for f in all_files if not re.match(r'native_(extra|pass2|retry)_', f)]
starts = [int(f[7:15], 16) for f in files]
# vtable-only functions from tools/ghidra/DecompileMissing.java
extra = {}
for f in all_files:
    if f.startswith('native_extra_'):
        with open(os.path.join(native_dir, f), encoding='utf-8') as fh:
            for line in fh:
                m = re.match(r'// ([0-9a-f]{8}) ', line)
                if m:
                    extra[int(m.group(1), 16)] = f


def file_of(addr):
    if addr in extra:
        return extra[addr]
    lo, hi = 0, len(starts) - 1
    best = files[0] if files else ''
    while lo <= hi:
        mid = (lo + hi) // 2
        if starts[mid] <= addr:
            best = files[mid]
            lo = mid + 1
        else:
            hi = mid - 1
    return best


funcs = []
with open(os.path.join(native_dir, 'functions.tsv'), encoding='utf-8') as fh:
    next(fh)
    for line in fh:
        a, name, size, refs = line.rstrip('\n').split('\t')
        funcs.append((int(a, 16), name, int(size), int(refs)))

# native function tables (tools/name_natives.py): "<CppClass>_exec<Name>"
exec_names = []
p = os.path.join(native_dir, 'exec_names.tsv')
if os.path.exists(p):
    with open(p, encoding='utf-8') as fh:
        next(fh)
        for line in fh:
            a, name, _ = line.rstrip('\n').split('\t')
            exec_names.append((int(a, 16), name))
n_exec = n_exec_cls = n_event = 0
with open(os.path.join(decomp, 'natives.tsv'), 'w', encoding='utf-8') as out:
    out.write('address\tnative\tfile\tscript class\tdeclaration\n')
    for a, name in exec_names:
        cpp_cls, fn = name.split('_exec', 1)
        script_cls = cpp_cls[1:]
        n_exec += 1
        decl = [d for d in native_decl.get(fn, []) if d[0].split('.', 1)[1] == script_cls] or native_decl.get(fn, [])
        n_exec_cls += bool(decl)
        out.write(f'{a:08x}\t{cpp_cls}::exec{fn}\t{file_of(a)}\t{",".join(c for c, _ in decl)}\t{decl[0][1] if decl else ""}\n')
with open(os.path.join(decomp, 'events.tsv'), 'w', encoding='utf-8') as out:
    out.write('address\tname\tfile\tclasses\n')
    for a, name, size, refs in funcs:
        if not name.startswith('event'):
            continue
        n_event += 1
        decl = event_decl.get(name[5:], [])
        out.write(f'{a:08x}\t{name}\t{file_of(a)}\t{",".join(c for c, _ in decl)}\n')

named = sum(1 for f in funcs if not f[1].startswith('FUN_'))
total_c = sum(os.path.getsize(os.path.join(native_dir, f)) for f in all_files)
managed = os.path.join(decomp, 'managed')
n_cs = sum(len([f for f in fs if f.endswith('.cs')]) for _, _, fs in os.walk(managed)) if os.path.isdir(managed) else 0

def rows(name):
    p = os.path.join(native_dir, name)
    return sum(1 for _ in open(p, encoding='utf-8')) - 1 if os.path.exists(p) else 0


n_cls, n_slots, n_thunks = rows('class_vtables.tsv'), rows('class_slots.tsv'), rows('thunks.tsv')
n_asm = len([f for f in os.listdir(native_dir) if f.startswith('asm_')])
with open(os.path.join(decomp, 'README.md'), 'w', encoding='utf-8') as out:
    out.write(f'''# Decompiled Goat Simulator (local analysis only — never distribute)

Generated from the user's own install. Nothing here is part of the GoatRust sources.

| What | Where | Count |
|---|---|---|
| UnrealScript (shipped source text) | `out/scripts/<Package>/<Class>.uc` | {n_scripts} classes |
| Native C++ (Ghidra 12.1.4 pseudo-C) | `native/native_<addr>.c` | {len(funcs)} functions, {len(all_files)} files, {total_c / 1e6:.0f} MB |
| Function index | `native/functions.tsv` | address, name, size, references |
| Names: tools/ghidra/DecompileAll.java (FName globals, eventXxx) + tools/name_natives.py (natives) | `native/names.tsv`, `native/exec_names.tsv` | {named} named functions |
| execXxx natives -> script classes | `natives.tsv` | {n_exec} natives, {n_exec_cls} matched to a script declaration |
| eventXxx thunks -> script classes | `events.tsv` | {n_event} |
| Managed (C++/CLI) part of the exe (ILSpy) | `managed/` | {n_cs} .cs files |
| Classes: registration, constructor, vtable, parent | `native/class_vtables.tsv` | {n_cls} classes |
| Virtual slots per class (resolve `(*param_1 + 0x524)`) | `native/class_slots.tsv` | {n_slots} rows |
| Thunks and their targets | `native/thunks.tsv` | {n_thunks} |
| Functions the decompiler cannot handle (assembly) | `native/asm_<addr>.s` | {n_asm} (editor UI, console Exec, script compiler) |

Passes: `native_<addr>.c` full run; `native_extra_*.c` vtable-only functions (DecompileMissing.java);
`native_pass2_*.c` orphan code / code after padding (SecondPass.java; a few are data misread as code);
`native_retry_*.c` earlier timeouts. Names: `Class_vSLOT[_known]` virtuals (tools/name_classes.py),
`Class_InternalConstructor`, `Class_vtable`, `CppClass_execName` natives (tools/name_natives.py),
`FN_*` FName globals and `eventXxx` thunks (DecompileAll.java).

Find a function: `grep -n "execMoveSmooth" natives.tsv`, then open the listed file and search for
`// <address>`. Every function starts with a `// <address> <name>` line.

The Ghidra project is kept in `out/ghidra/GoatGame.gpr`: rerun scripts without re-analysing with
`analyzeHeadless out/ghidra GoatGame -process -noanalysis -scriptPath tools/ghidra -postScript ...`.
''')
print(f'{n_scripts} scripts, {len(funcs)} functions ({named} named), {n_exec} exec ({n_exec_cls} matched), {n_event} events, {n_cs} .cs')
