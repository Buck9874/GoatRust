// Second decompile pass over the analysed GoatGame.exe project:
//   1. orphan code: instructions in .text outside every function, and never-disassembled code
//      that starts right after int3/nop padding, become functions;
//   2. functions whose earlier decompile failed (timeouts) are retried with a long timeout;
//   3. thunks (skipped before) are listed in thunks.tsv with their targets.
// args[0] = out/decomp/native. Writes native_pass2_<n>.c, appends functions.tsv, writes thunks.tsv.
//@category GoatRust
import ghidra.app.cmd.function.CreateFunctionCmd;
import ghidra.app.decompiler.*;
import ghidra.app.decompiler.parallel.*;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.mem.*;
import ghidra.util.task.TaskMonitor;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.nio.file.*;
import java.util.*;
import java.util.regex.*;

public class SecondPass extends GhidraScript {
    record Out(long addr, String text) {}

    @Override
    public void run() throws Exception {
        Path out = Paths.get(getScriptArgs()[0]);
        Memory mem = currentProgram.getMemory();
        MemoryBlock text = mem.getBlock(".text");
        FunctionManager fm = currentProgram.getFunctionManager();
        Listing listing = currentProgram.getListing();
        AddressSet textSet = new AddressSet(text.getStart(), text.getEnd());

        // --- 1a. instructions outside functions -------------------------------------------------
        AddressSet bodies = new AddressSet();
        for (Function f : fm.getFunctions(true)) bodies.add(f.getBody());
        AddressSetView rest = textSet.subtract(bodies);
        List<Function> created = new ArrayList<>();
        Address runEnd = null;
        int orphanRuns = 0;
        InstructionIterator ii = listing.getInstructions(rest, true);
        while (ii.hasNext() && !monitor.isCancelled()) {
            Instruction ins = ii.next();
            Address a = ins.getAddress();
            if (runEnd != null && a.equals(runEnd)) { runEnd = ins.getMaxAddress().next(); continue; }
            runEnd = ins.getMaxAddress().next();
            orphanRuns++;
            if (fm.getFunctionContaining(a) != null) continue;
            if (new CreateFunctionCmd(a).applyTo(currentProgram, monitor)) {
                Function f = fm.getFunctionAt(a);
                if (f != null) created.add(f);
            }
        }
        println("orphan instruction runs: " + orphanRuns + ", functions created: " + created.size());

        // --- 1b. undisassembled code after padding ----------------------------------------------
        int before = created.size();
        byte[] buf = new byte[(int) text.getSize()];
        text.getBytes(text.getStart(), buf);
        long base = text.getStart().getOffset();
        for (int i = 16; i < buf.length - 16; i += 16) {
            if (monitor.isCancelled()) break;
            byte prev = buf[i - 1];
            if (prev != (byte) 0xcc && prev != (byte) 0x90 && prev != (byte) 0xc3) continue;
            if (buf[i] == (byte) 0xcc || buf[i] == 0) continue;
            Address a = toAddr(base + i);
            if (fm.getFunctionContaining(a) != null) continue;
            if (listing.getDefinedDataContaining(a) != null) continue;
            if (listing.getInstructionAt(a) == null) {
                if (!disassemble(a)) continue;
            }
            if (new CreateFunctionCmd(a).applyTo(currentProgram, monitor)) {
                Function f = fm.getFunctionAt(a);
                if (f != null) created.add(f);
            }
        }
        println("functions after padding: " + (created.size() - before));

        // --- 2. previous decompile failures -------------------------------------------------------
        Pattern failed = Pattern.compile("^// ([0-9a-f]{8}) \\S+\\n/\\* decompile failed", Pattern.MULTILINE);
        Set<Long> retry = new TreeSet<>();
        try (DirectoryStream<Path> ds = Files.newDirectoryStream(out, "native_*.c")) {
            for (Path p : ds) {
                Matcher m = failed.matcher(Files.readString(p, StandardCharsets.UTF_8));
                while (m.find()) retry.add(Long.parseLong(m.group(1), 16));
            }
        }
        List<Function> retryFns = new ArrayList<>();
        for (long r : retry) {
            Function f = fm.getFunctionAt(toAddr(r));
            if (f != null) retryFns.add(f);
        }
        println("retrying " + retryFns.size() + " failed decompiles");

        // --- 3. thunks ----------------------------------------------------------------------------
        int nThunks = 0;
        try (PrintWriter tw = new PrintWriter(Files.newBufferedWriter(out.resolve("thunks.tsv"), StandardCharsets.UTF_8))) {
            tw.println("address\tname\ttarget\ttarget_name");
            for (Function f : fm.getFunctions(true)) {
                if (!f.isThunk()) continue;
                Function t = f.getThunkedFunction(true);
                tw.println(f.getEntryPoint() + "\t" + f.getName() + "\t" + (t == null ? "" : t.getEntryPoint()) + "\t" + (t == null ? "" : t.getName()));
                nThunks++;
            }
        }
        println("thunks: " + nThunks);

        try (PrintWriter idx = new PrintWriter(new FileWriter(out.resolve("functions.tsv").toFile(), StandardCharsets.UTF_8, true))) {
            for (Function f : created) {
                idx.println(f.getEntryPoint() + "\t" + f.getName() + "\t" + f.getBody().getNumAddresses() + "\t" + currentProgram.getReferenceManager().getReferenceCountTo(f.getEntryPoint()));
            }
        }
        decompile(out, created, "native_pass2_%02d.c", 120);
        decompile(out, retryFns, "native_retry_%02d.c", 900);
    }

    void decompile(Path out, List<Function> fns, String pattern, int timeout) throws Exception {
        if (fns.isEmpty()) return;
        DecompileConfigurer conf = d -> {
            DecompileOptions o = new DecompileOptions();
            o.grabFromProgram(currentProgram);
            o.setMaxPayloadMBytes(256);
            d.setOptions(o);
            d.toggleCCode(true);
            d.toggleSyntaxTree(false);
            d.setSimplificationStyle("decompile");
        };
        DecompilerCallback<Out> cb = new DecompilerCallback<>(currentProgram, conf) {
            @Override
            public Out process(DecompileResults r, TaskMonitor m) {
                Function f = r.getFunction();
                String body = r.decompileCompleted() && r.getDecompiledFunction() != null
                    ? r.getDecompiledFunction().getC()
                    : "/* decompile failed: " + r.getErrorMessage() + " */\n";
                return new Out(f.getEntryPoint().getOffset(), "// " + f.getEntryPoint() + " " + f.getName() + "\n" + body + "\n");
            }
        };
        cb.setTimeout(timeout);
        int batch = 4000, n = 0;
        try {
            for (int i = 0; i < fns.size(); i += batch, n++) {
                List<Function> part = fns.subList(i, Math.min(fns.size(), i + batch));
                List<Out> res = new ArrayList<>(ParallelDecompiler.decompileFunctions(cb, part, monitor));
                res.removeIf(Objects::isNull);
                res.sort(Comparator.comparingLong(Out::addr));
                long failedCount = res.stream().filter(o -> o.text().contains("decompile failed")).count();
                try (Writer w = Files.newBufferedWriter(out.resolve(String.format(pattern, n)), StandardCharsets.UTF_8)) {
                    for (Out o : res) w.write(o.text());
                }
                println(String.format("%s: decompiled %d/%d (%d failed)", pattern, Math.min(fns.size(), i + batch), fns.size(), failedCount));
            }
        } finally {
            cb.dispose();
        }
    }
}
