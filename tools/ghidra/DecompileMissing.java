// Second pass over the analysed GoatGame.exe project: functions only reachable through vtables
// (no direct calls) were never created by auto-analysis. Every pointer in .rdata/.data that lands
// in .text outside an existing function becomes a function, and those are decompiled.
//
// args[0] = output directory (out/decomp/native). Writes native_extra_<n>.c and appends to
// functions.tsv.
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

public class DecompileMissing extends GhidraScript {
    record Out(long addr, String text) {}

    @Override
    public void run() throws Exception {
        Path out = Paths.get(getScriptArgs()[0]);
        Memory mem = currentProgram.getMemory();
        MemoryBlock text = mem.getBlock(".text");
        FunctionManager fm = currentProgram.getFunctionManager();
        Listing listing = currentProgram.getListing();
        TreeSet<Long> targets = new TreeSet<>();
        for (MemoryBlock b : mem.getBlocks()) {
            if (!b.isInitialized() || b.isExecute()) continue;
            if (!(b.getName().equals(".rdata") || b.getName().equals(".data"))) continue;
            byte[] buf = new byte[(int) b.getSize()];
            b.getBytes(b.getStart(), buf);
            for (int i = 0; i + 4 <= buf.length; i += 4) {
                long p = (buf[i] & 0xffL) | (buf[i + 1] & 0xffL) << 8 | (buf[i + 2] & 0xffL) << 16 | (buf[i + 3] & 0xffL) << 24;
                if (p < text.getStart().getOffset() || p >= text.getEnd().getOffset()) continue;
                // functions start 16-byte aligned in this MSVC build
                if ((p & 0xf) != 0) continue;
                targets.add(p);
            }
        }
        println("code pointers in data: " + targets.size());
        List<Function> created = new ArrayList<>();
        int skipped = 0;
        for (long p : targets) {
            if (monitor.isCancelled()) break;
            Address a = toAddr(p);
            if (fm.getFunctionContaining(a) != null) continue;
            Data d = listing.getDefinedDataAt(a);
            if (d != null) { skipped++; continue; }
            if (listing.getInstructionAt(a) == null) {
                if (!disassemble(a)) { skipped++; continue; }
            }
            CreateFunctionCmd cmd = new CreateFunctionCmd(a);
            if (cmd.applyTo(currentProgram, monitor)) {
                Function f = fm.getFunctionAt(a);
                if (f != null) created.add(f);
            } else {
                skipped++;
            }
        }
        println("created " + created.size() + " functions (" + skipped + " pointers skipped)");
        try (PrintWriter idx = new PrintWriter(new FileWriter(out.resolve("functions.tsv").toFile(), StandardCharsets.UTF_8, true))) {
            for (Function f : created) {
                idx.println(f.getEntryPoint() + "\t" + f.getName() + "\t" + f.getBody().getNumAddresses() + "\t" + currentProgram.getReferenceManager().getReferenceCountTo(f.getEntryPoint()));
            }
        }
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
                String body = r.decompileCompleted() && r.getDecompiledFunction() != null
                    ? r.getDecompiledFunction().getC()
                    : "/* decompile failed: " + r.getErrorMessage() + " */\n";
                return new Out(f.getEntryPoint().getOffset(), "// " + f.getEntryPoint() + " " + f.getName() + "\n" + body + "\n");
            }
        };
        cb.setTimeout(60);
        int batch = 4000, n = 0;
        try {
            for (int i = 0; i < created.size(); i += batch, n++) {
                List<Function> part = created.subList(i, Math.min(created.size(), i + batch));
                List<Out> res = new ArrayList<>(ParallelDecompiler.decompileFunctions(cb, part, monitor));
                res.removeIf(Objects::isNull);
                res.sort(Comparator.comparingLong(Out::addr));
                try (Writer w = Files.newBufferedWriter(out.resolve(String.format("native_extra_%02d.c", n)), StandardCharsets.UTF_8)) {
                    for (Out o : res) w.write(o.text());
                }
                println(String.format("decompiled %d/%d", Math.min(created.size(), i + batch), created.size()));
            }
        } finally {
            cb.dispose();
        }
    }
}
