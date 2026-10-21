"""Name virtual functions after their classes.

Every native UClass is registered as
    FUN_005aa4a0(0, size, flags, 0, L"<Class>", .., L"<Package>", .., .., <InternalConstructor>, ..)
and its InternalConstructor stores the class's vtable (`*param_1 = &PTR_FUN_<vtable>`). Walking each
vtable gives (class, slot) for every virtual function; a function shared by several classes at a
slot is named after the class with the shortest vtable (the most basic one), e.g.
`Pawn_v258_physWalking` / `GGPawnBase_v550`. Known slots get their UE3 names.

Writes out/decomp/native/class_vtables.tsv and class_names.tsv; with --apply renames FUN_<addr>
and PTR_FUN_<vtable> in every .c file and in functions.tsv.

usage: python -I tools/name_classes.py [--apply]
"""
import collections
import os
import re
import struct
import sys

TAB, NL = chr(9), chr(10)

root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
nd = os.path.join(root, 'out', 'decomp', 'native')
exe = os.path.join(root, 'out', 'rea', 'GoatGame-native.exe')
apply = '--apply' in sys.argv

data = open(exe, 'rb').read()
pe = struct.unpack_from('<I', data, 0x3c)[0]
nsec = struct.unpack_from('<H', data, pe + 6)[0]
opt = struct.unpack_from('<H', data, pe + 20)[0]
image_base = struct.unpack_from('<I', data, pe + 24 + 28)[0]
secs = []
for i in range(nsec):
    o = pe + 24 + opt + i * 40
    name = data[o:o + 8].rstrip(b'\0').decode('latin1')
    vs, va, rs, raw = struct.unpack_from('<IIII', data, o + 8)
    secs.append((name, image_base + va, vs, raw))
text = next(s for s in secs if s[0] == '.text')


def rd(v):
    for _, va, vs, raw in secs:
        if va <= v < va + vs:
            return struct.unpack_from('<I', data, raw + v - va)[0]
    return None


def in_text(v):
    return v is not None and text[1] <= v < text[1] + text[2]


files = sorted(f for f in os.listdir(nd) if f.startswith('native_') and f.endswith('.c'))

# function bodies by address (only those we need: constructors)
reg_re = re.compile(r'FUN_005aa4a0\(([^;]*?)\);', re.S)
name_addr = {}
with open(os.path.join(nd, 'functions.tsv'), encoding='utf-8') as fh:
    next(fh)
    for line in fh:
        a_, n_ = line.split(TAB)[:2]
        name_addr.setdefault(n_, int(a_, 16))
classes = {}  # class -> (package, ctor)
bodies = {}
func_re = re.compile(r'^// ([0-9a-f]{8}) \S+\n', re.M)
for f in files:
    s = open(os.path.join(nd, f), encoding='utf-8').read()
    for m in reg_re.finditer(s):
        args = m.group(1)
        names = re.findall(r'L"([A-Za-z0-9_]+)"', args)
        fns = re.findall(r'FUN_([0-9a-f]{8})', args) or [f'{a:08x}' for a in (name_addr.get(n) for n in re.findall(r'(\w+_InternalConstructor)', args)) if a]
        if len(names) >= 2 and fns:
            classes.setdefault(names[0], (names[1], int(fns[0], 16)))
    # keep every function body, keyed by address (constructors are looked up below)
    starts = [(int(m.group(1), 16), m.start()) for m in func_re.finditer(s)]
    for i, (a, st) in enumerate(starts):
        end = starts[i + 1][1] if i + 1 < len(starts) else len(s)
        bodies[a] = s[st:end] if len(s) - st < 50_000_000 else ''
print(f'{len(classes)} registered classes')

def vtable_from_code(ctor):
    """The last `mov dword [reg], imm32` in the constructor whose imm32 is a table of code pointers."""
    found = None
    for _, va, vs, raw in secs:
        if va <= ctor < va + vs:
            code = data[raw + ctor - va: raw + ctor - va + 400]
            for i in range(len(code) - 6):
                if code[i] == 0xC7 and code[i + 1] in (0x00, 0x01, 0x02, 0x03, 0x06, 0x07):
                    imm = struct.unpack_from('<I', code, i + 2)[0]
                    if in_text(rd(imm)) and in_text(rd(imm + 4)) and not in_text(imm):
                        found = imm
                if code[i] == 0xC3 or (code[i] == 0xC2 and i > 0):
                    break
    return found


vt_re = re.compile(r'\*param_1 = &(?:PTR_(?:FUN|LAB)_([0-9a-f]{8})|(\w+)_vtable);')
# a previous --apply renamed PTR_FUN_<vtable> to <Class>_vtable: read the old map back
known_vt = {}
prev = os.path.join(nd, 'class_vtables.tsv')
if os.path.exists(prev):
    with open(prev, encoding='utf-8') as fh:
        next(fh)
        for line in fh:
            c = line.rstrip(NL).split(TAB)
            if len(c) > 3 and c[3]:
                known_vt[c[0]] = int(c[3], 16)
vtables = {}
for cls, (pkg, ctor) in classes.items():
    b = bodies.get(ctor, '')
    hits = vt_re.findall(b)
    vt = None
    if hits:
        addr, name = hits[-1]
        vt = int(addr, 16) if addr else known_vt.get(name)
    vt = vt or vtable_from_code(ctor)
    if vt:
        vtables[cls] = vt
print(f'{len(vtables)} vtables')

starts_set = set(vtables.values())
slots = {}
for cls, vt in vtables.items():
    entries = []
    a = vt
    while True:
        p = rd(a)
        if not in_text(p) or (a != vt and a in starts_set) or len(entries) > 4000:
            break
        entries.append(p)
        a += 4
    slots[cls] = entries

# class hierarchy from the shipped scripts (class X extends Y)
parent = {}
scripts = os.path.join(root, 'out', 'scripts')
for pkg in os.listdir(scripts):
    d = os.path.join(scripts, pkg)
    if not os.path.isdir(d):
        continue
    for f in os.listdir(d):
        src = open(os.path.join(d, f), encoding='utf-8', errors='replace').read()
        m = re.search(r'^class\s+(\w+)\s+extends\s+(\w+)', src, re.M)
        if m:
            parent[m.group(1)] = m.group(2)


def ancestors(c):
    out = []
    while c in parent and len(out) < 40:
        c = parent[c]
        out.append(c)
    return out


# UE3 virtuals whose slots were identified (see Docs/RESUME.md); GG's own virtuals only exist
# from GGPawnBase down
pawn_slots = {0x254: 'physFalling', 0x258: 'physWalking', 0x264: 'physWallRun', 0x4e8: 'startNewPhysics',
              0x500: 'NewFallVelocity', 0x504: 'MaxSpeedModifier', 0x508: 'GetMaxAccel', 0x524: 'ApplyBraking',
              0x528: 'CalcVelocity'}
ggpawn_slots = {0x550: 'CalcVelocityWalking', 0x554: 'CalcVelocitySwimming', 0x558: 'CalcVelocitySpider',
                0x55c: 'GetMaxSpeedInDirection'}

# a virtual belongs to the most basic class (up the extends chain) with it in the same slot
owner = {}
for cls, entries in slots.items():
    anc = ancestors(cls)
    for i, fa in enumerate(entries):
        top = cls
        for a in anc:
            e = slots.get(a)
            if e is None:
                continue  # script-only class in between: no vtable of its own
            if i < len(e) and e[i] == fa:
                top = a
            else:
                break
        tchain = [top] + ancestors(top)
        known = pawn_slots.get(i * 4) if 'Pawn' in tchain else None
        if 'GGPawnBase' in tchain:
            known = known or ggpawn_slots.get(i * 4)
        cand = (len(tchain), top, i * 4, known)
        cur = owner.get(fa)
        # identical-code-folded bodies shared by unrelated classes: keep the most basic owner
        if cur is None or cand[0] < cur[0]:
            owner[fa] = cand

with open(os.path.join(nd, 'class_vtables.tsv'), 'w', encoding='utf-8') as fh:
    fh.write('class\tconfig\tconstructor\tvtable\tslots\tparent\n')
    for cls in sorted(classes):
        cfg, ctor = classes[cls]
        vt = vtables.get(cls)
        par = parent.get(cls, '')
        fh.write(f'{cls}\t{cfg}\t{ctor:08x}\t{vt:08x}\t{len(slots.get(cls, []))}\t{par}\n' if vt else f'{cls}\t{cfg}\t{ctor:08x}\t\t0\t{par}\n')

current = {}
with open(os.path.join(nd, 'functions.tsv'), encoding='utf-8') as fh:
    next(fh)
    for line in fh:
        a, n = line.split('\t')[:2]
        current[int(a, 16)] = n

rename = {}
for fa, (_, cls, slot, known) in owner.items():
    cur = current.get(fa, 'FUN_')
    if not (cur.startswith('FUN_') or re.match(r'\w+_v[0-9a-f]{3}', cur)):
        continue
    rename[f'FUN_{fa:08x}'] = f'{cls}_v{slot:03x}' + (f'_{known}' if known else '')
for cls, (pkg, ctor) in classes.items():
    if current.get(ctor, 'FUN_').startswith('FUN_'):
        rename[f'FUN_{ctor:08x}'] = f'{cls}_InternalConstructor'
vt_names = {f'PTR_FUN_{vt:08x}': f'{cls}_vtable' for cls, vt in vtables.items()}
vt_names.update({f'PTR_LAB_{vt:08x}': f'{cls}_vtable' for cls, vt in vtables.items()})
with open(os.path.join(nd, 'class_names.tsv'), 'w', encoding='utf-8') as fh:
    fh.write('old\tnew\n')
    for k in sorted(rename):
        fh.write(f'{k}\t{rename[k]}\n')
print(f'{len(rename)} functions named by class/slot, {len(vt_names)} vtable labels')

# every class's slot -> function, so `(*param_1 + 0x524)` calls can be looked up
final = {int(k[4:], 16): v for k, v in rename.items()}
with open(os.path.join(nd, 'class_slots.tsv'), 'w', encoding='utf-8') as fh:
    fh.write('class' + TAB + 'slot' + TAB + 'address' + TAB + 'function' + NL)
    for cls in sorted(slots):
        for i, fa in enumerate(slots[cls]):
            name = final.get(fa, current.get(fa, f'FUN_{fa:08x}'))
            fh.write(TAB.join([cls, f'{i * 4:#05x}', f'{fa:08x}', name]) + NL)


if apply:
    allmap = {**rename, **vt_names}
    pat = re.compile(r'\b(?:FUN|PTR_FUN|PTR_LAB)_[0-9a-f]{8}\b')
    skip = {x.split('=', 1)[1] for x in sys.argv if x.startswith('--skip=')}
    for f in [x for x in files if x not in skip] + ['functions.tsv']:
        p = os.path.join(nd, f)
        s = open(p, encoding='utf-8').read()
        t = pat.sub(lambda m: allmap.get(m.group(), m.group()), s)
        if t != s:
            open(p, 'w', encoding='utf-8').write(t)
    print('applied')
