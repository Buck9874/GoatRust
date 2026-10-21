// Pre-script: skip analyzers that make the 32 MB GoatGame.exe take hours.
//@category GoatRust
import ghidra.app.script.GhidraScript;

public class DisableSlowAnalyzers extends GhidraScript {
    @Override
    public void run() throws Exception {
        setAnalysisOption(currentProgram, "Decompiler Parameter ID", "false");
        println("disabled Decompiler Parameter ID");
    }
}
