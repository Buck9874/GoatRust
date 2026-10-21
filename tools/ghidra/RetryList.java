// Decompile a list of functions (one hex address per line in args[1]) with a long timeout and
// write them to args[0]/native_retry_<args[2]>.c.
//@category GoatRust
import ghidra.app.decompiler.*;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.listing.*;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.nio.file.*;

public class RetryList extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        Path out = Paths.get(a[0]);
        DecompInterface d = new DecompInterface();
        DecompileOptions o = new DecompileOptions();
        o.grabFromProgram(currentProgram);
        o.setMaxPayloadMBytes(512);
        o.setMaxInstructions(500000);
        d.setOptions(o);
        d.toggleCCode(true);
        d.toggleSyntaxTree(false);
        d.setSimplificationStyle("decompile");
        d.openProgram(currentProgram);
        try (Writer w = Files.newBufferedWriter(out.resolve("native_retry_" + a[2] + ".c"), StandardCharsets.UTF_8)) {
            for (String line : Files.readAllLines(Paths.get(a[1]))) {
                if (line.isBlank()) continue;
                Function f = getFunctionAt(toAddr(Long.parseLong(line.trim(), 16)));
                if (f == null) continue;
                long t0 = System.currentTimeMillis();
                DecompileResults r = d.decompileFunction(f, 1800, monitor);
                boolean ok = r.decompileCompleted() && r.getDecompiledFunction() != null;
                w.write("// " + f.getEntryPoint() + " " + f.getName() + "\n");
                w.write(ok ? r.getDecompiledFunction().getC() : "/* decompile failed: " + r.getErrorMessage() + " */\n");
                w.write("\n");
                w.flush();
                println(f.getEntryPoint() + " " + (ok ? "ok" : "failed: " + r.getErrorMessage()) + " in " + (System.currentTimeMillis() - t0) / 1000 + " s");
            }
        } finally {
            d.dispose();
        }
    }
}
