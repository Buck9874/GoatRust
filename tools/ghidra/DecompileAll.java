// Post-script: name what UE3 lets us name, then decompile every function of GoatGame.exe.
//
// Naming (all USER_DEFINED, so later runs and the decompiler output show them):
//   * execXxx      - native function tables ({ "execXxx", &Class::execXxx } pairs in .data/.rdata)
//   * FN_Xxx       - FName globals set up in the registration functions
//                    (a wide identifier string pushed next to a store into a .data global)
//   * eventXxx     - small functions using exactly one FN_ global (the eventXxx script thunks)
//
// Output (args[0] = output directory):
//   native_<addr>.c   - decompiled C, one file per batch of functions in address order
//   functions.tsv     - address, name, body size, references to it
//   names.tsv         - every name this script created
//@category GoatRust
import ghidra.app.decompiler.*;
import ghidra.app.decompiler.parallel.*;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.mem.*;
import ghidra.program.model.symbol.*;
import ghidra.util.task.TaskMonitor;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.nio.file.*;
import java.util.*;
import java.util.regex.Pattern;

public class DecompileAll extends GhidraScript {
    static final Pattern IDENT = Pattern.compile("[A-Za-z_][A-Za-z0-9_]{1,80}");
    PrintWriter names;
    int named = 0;

    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        Path out = Paths.get(args.length > 0 ? args[0] : "decomp");
        Files.createDirectories(out);
        names = new PrintWriter(Files.newBufferedWriter(out.resolve("names.tsv"), StandardCharsets.UTF_8));
        long t0 = System.currentTimeMillis();
        nameExecTables();
        println("exec names: " + named + " in " + (System.currentTimeMillis() - t0) / 1000 + " s");
        int before = named;
        Map<Address, String> fnames = nameFNameGlobals();
        println("FName globals: " + (named - before));
        before = named;
        nameEventWrappers(fnames);
        println("event wrappers: " + (named - before));
        names.close();
        decompileAll(out);
        println("done in " + (System.currentTimeMillis() - t0) / 1000 + " s");
    }

    void record(Address a, String name, String kind) {
        names.println(a + "\t" + name + "\t" + kind);
        named++;
    }

    /** ANSI "execXxx" strings referenced by a pointer that is followed by a code pointer. */
    void nameExecTables() throws Exception {
        Map<Long, String> execStrings = new HashMap<>();
        DataIterator it = currentProgram.getListing().getDefinedData(true);
        while (it.hasNext() && !monitor.isCancelled()) {
            Data d = it.next();
            if (!d.hasStringValue()) continue;
            Object v = d.getValue();
            if (!(v instanceof String s)) continue;
            if (s.startsWith("exec") && IDENT.matcher(s).matches() && !d.getDataType().getName().toLowerCase().contains("unicode")) {
                execStrings.put(d.getAddress().getOffset(), s);
            }
        }
        println("exec strings: " + execStrings.size());
        Memory mem = currentProgram.getMemory();
        FunctionManager fm = currentProgram.getFunctionManager();
        for (MemoryBlock b : mem.getBlocks()) {
            if (!b.isInitialized() || b.isExecute()) continue;
            if (!(b.getName().equals(".data") || b.getName().equals(".rdata"))) continue;
            byte[] buf = new byte[(int) b.getSize()];
            b.getBytes(b.getStart(), buf);
            for (int i = 0; i + 8 <= buf.length; i += 4) {
                long p = (buf[i] & 0xffL) | (buf[i + 1] & 0xffL) << 8 | (buf[i + 2] & 0xffL) << 16 | (buf[i + 3] & 0xffL) << 24;
                String s = execStrings.get(p);
                if (s == null) continue;
                long q = (buf[i + 4] & 0xffL) | (buf[i + 5] & 0xffL) << 8 | (buf[i + 6] & 0xffL) << 16 | (buf[i + 7] & 0xffL) << 24;
                Address fa = toAddr(q);
                MemoryBlock fb = mem.getBlock(fa);
                if (fb == null || !fb.isExecute()) continue;
                Function f = fm.getFunctionAt(fa);
                if (f == null) {
                    disassemble(fa);
                    f = createFunction(fa, null);
                }
                if (f != null && f.getName().startsWith("FUN_")) {
                    f.setName(s, SourceType.USER_DEFINED);
                    record(fa, s, "exec");
                }
            }
        }
    }

    boolean inData(Address a) {
        MemoryBlock b = a == null ? null : currentProgram.getMemory().getBlock(a);
        return b != null && b.getName().equals(".data");
    }

    /** Wide identifier strings whose code reference sits next to a .data global (FName init). */
    Map<Address, String> nameFNameGlobals() throws Exception {
        Map<Address, String> globals = new HashMap<>();
        Listing listing = currentProgram.getListing();
        SymbolTable st = currentProgram.getSymbolTable();
        DataIterator it = listing.getDefinedData(true);
        while (it.hasNext() && !monitor.isCancelled()) {
            Data d = it.next();
            if (!d.hasStringValue() || !d.getDataType().getName().toLowerCase().contains("unicode")) continue;
            Object v = d.getValue();
            if (!(v instanceof String s) || !IDENT.matcher(s).matches()) continue;
            for (Reference r : getReferencesTo(d.getAddress())) {
                Instruction ins = listing.getInstructionAt(r.getFromAddress());
                if (ins == null) continue;
                Address g = findGlobalNear(ins);
                if (g == null || globals.containsKey(g)) continue;
                globals.put(g, s);
                Symbol sym = st.getPrimarySymbol(g);
                if (sym == null || sym.getSource() != SourceType.USER_DEFINED) {
                    try {
                        createLabel(g, "FN_" + s, true, SourceType.USER_DEFINED);
                        record(g, "FN_" + s, "fname");
                    } catch (Exception e) {
                        // name clash: keep going
                    }
                }
            }
        }
        return globals;
    }

    /** MOV ECX,<global> just before (thiscall ctor) or MOV [<global>],reg just after the call. */
    Address findGlobalNear(Instruction ins) {
        Instruction p = ins;
        for (int k = 0; k < 4 && p != null; k++) {
            p = p.getPrevious();
            if (p == null) break;
            if (p.getMnemonicString().equals("MOV") && p.getNumOperands() == 2 && p.getRegister(0) != null && p.getRegister(0).getName().equals("ECX")) {
                for (Reference r : p.getOperandReferences(1)) {
                    if (inData(r.getToAddress())) return r.getToAddress();
                }
                Object[] objs = p.getOpObjects(1);
                if (objs.length == 1 && objs[0] instanceof ghidra.program.model.scalar.Scalar sc) {
                    Address a = toAddr(sc.getUnsignedValue());
                    if (inData(a)) return a;
                }
            }
        }
        Instruction n = ins;
        boolean afterCall = false;
        for (int k = 0; k < 6 && n != null; k++) {
            n = n.getNext();
            if (n == null) break;
            if (n.getFlowType().isCall()) {
                afterCall = true;
                continue;
            }
            if (afterCall && n.getMnemonicString().equals("MOV")) {
                for (Reference r : n.getOperandReferences(0)) {
                    if (r.getReferenceType().isWrite() && inData(r.getToAddress())) return r.getToAddress();
                }
            }
        }
        return null;
    }

    /** Small functions that read exactly one FName global: the eventXxx thunks into script. */
    void nameEventWrappers(Map<Address, String> fnames) throws Exception {
        Map<Function, Set<String>> uses = new HashMap<>();
        FunctionManager fm = currentProgram.getFunctionManager();
        for (Map.Entry<Address, String> e : fnames.entrySet()) {
            for (Reference r : getReferencesTo(e.getKey())) {
                if (r.getReferenceType().isWrite()) continue;
                Function f = fm.getFunctionContaining(r.getFromAddress());
                if (f != null) uses.computeIfAbsent(f, x -> new HashSet<>()).add(e.getValue());
            }
        }
        for (Map.Entry<Function, Set<String>> e : uses.entrySet()) {
            Function f = e.getKey();
            if (e.getValue().size() != 1 || !f.getName().startsWith("FUN_") || f.getBody().getNumAddresses() > 400) continue;
            String n = "event" + e.getValue().iterator().next();
            try {
                f.setName(n, SourceType.USER_DEFINED);
                record(f.getEntryPoint(), n, "event");
            } catch (Exception ex) {
                // duplicate name in namespace
            }
        }
    }

    record Out(long addr, String text) {}

    void decompileAll(Path out) throws Exception {
        FunctionManager fm = currentProgram.getFunctionManager();
        List<Function> all = new ArrayList<>();
        try (PrintWriter idx = new PrintWriter(Files.newBufferedWriter(out.resolve("functions.tsv"), StandardCharsets.UTF_8))) {
            idx.println("address\tname\tsize\trefs");
            ReferenceManager rm = currentProgram.getReferenceManager();
            for (Function f : fm.getFunctions(true)) {
                if (f.isExternal()) continue;
                idx.println(f.getEntryPoint() + "\t" + f.getName() + "\t" + f.getBody().getNumAddresses() + "\t" + rm.getReferenceCountTo(f.getEntryPoint()));
                if (!f.isThunk()) all.add(f);
            }
        }
        println("decompiling " + all.size() + " functions");
        DecompileConfigurer conf = d -> {
            DecompileOptions o = new DecompileOptions();
            o.grabFromProgram(currentProgram);
            d.setOptions(o);
            d.toggleCCode(true);
            d.toggleSyntaxTree(false);
            d.setSimplificationStyle("decompile");
        };
        DecompilerCallback<Out> cb = new DecompilerCallback<>(currentProgram, conf) {
            @Override
            public Out process(DecompileResults r, TaskMonitor m) {
                Function f = r.getFunction();
                String body;
                if (r.decompileCompleted() && r.getDecompiledFunction() != null) {
                    body = r.getDecompiledFunction().getC();
                } else {
                    body = "/* decompile failed: " + r.getErrorMessage() + " */\n";
                }
                return new Out(f.getEntryPoint().getOffset(), "// " + f.getEntryPoint() + " " + f.getName() + "\n" + body + "\n");
            }
        };
        cb.setTimeout(60);
        int batch = 4000;
        long t0 = System.currentTimeMillis();
        try {
            for (int i = 0; i < all.size() && !monitor.isCancelled(); i += batch) {
                List<Function> part = all.subList(i, Math.min(all.size(), i + batch));
                List<Out> res = new ArrayList<>(ParallelDecompiler.decompileFunctions(cb, part, monitor));
                res.removeIf(Objects::isNull);
                res.sort(Comparator.comparingLong(Out::addr));
                Path file = out.resolve(String.format("native_%08x.c", part.get(0).getEntryPoint().getOffset()));
                try (Writer w = Files.newBufferedWriter(file, StandardCharsets.UTF_8)) {
                    for (Out o : res) w.write(o.text());
                }
                long s = (System.currentTimeMillis() - t0) / 1000;
                println(String.format("decompiled %d/%d (%d s)", Math.min(all.size(), i + batch), all.size(), s));
            }
        } finally {
            cb.dispose();
        }
    }
}
