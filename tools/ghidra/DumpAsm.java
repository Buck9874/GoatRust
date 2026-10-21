// Write the assembly of a list of functions (hex addresses in args[1]) to args[0]/asm_<addr>.s,
// for functions the decompiler cannot handle.
//@category GoatRust
import ghidra.app.script.GhidraScript;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.nio.file.*;

public class DumpAsm extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        Path out = Paths.get(a[0]);
        Listing listing = currentProgram.getListing();
        for (String line : Files.readAllLines(Paths.get(a[1]))) {
            if (line.isBlank()) continue;
            Function f = getFunctionAt(toAddr(Long.parseLong(line.trim(), 16)));
            if (f == null) continue;
            try (Writer w = Files.newBufferedWriter(out.resolve("asm_" + line.trim() + ".s"), StandardCharsets.UTF_8)) {
                w.write("; " + f.getEntryPoint() + " " + f.getName() + " (" + f.getBody().getNumAddresses() + " bytes)\n");
                for (Instruction ins : listing.getInstructions(f.getBody(), true)) {
                    Symbol s = currentProgram.getSymbolTable().getPrimarySymbol(ins.getAddress());
                    if (s != null && !s.getAddress().equals(f.getEntryPoint())) w.write(s.getName() + ":\n");
                    StringBuilder refs = new StringBuilder();
                    for (Reference r : ins.getReferencesFrom()) {
                        Symbol t = currentProgram.getSymbolTable().getPrimarySymbol(r.getToAddress());
                        if (t != null) refs.append(' ').append(t.getName());
                        Data d = listing.getDefinedDataAt(r.getToAddress());
                        if (d != null && d.hasStringValue()) refs.append(" \"").append(String.valueOf(d.getValue()).replace("\n", "\\n")).append('"');
                    }
                    w.write(String.format("  %s  %-40s%s%n", ins.getAddress(), ins.toString(), refs.length() > 0 ? " ;" + refs : ""));
                }
            }
            println("wrote asm for " + f.getEntryPoint());
        }
    }
}
