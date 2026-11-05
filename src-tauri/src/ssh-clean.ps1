param([string[]] $SshArgs)

# Use a native console child, not PowerShell's pipeline/native-argument conversion.
Add-Type -TypeDefinition @'
using System;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;

public static class TunnelBarSshClean {
    [DllImport("kernel32.dll")] static extern IntPtr GetStdHandle(int handle);
    [DllImport("kernel32.dll")] static extern bool GetConsoleMode(IntPtr handle, out uint mode);
    [DllImport("kernel32.dll")] static extern bool SetConsoleMode(IntPtr handle, uint mode);
    delegate bool ControlHandler(uint type);
    [DllImport("kernel32.dll")] static extern bool SetConsoleCtrlHandler(ControlHandler handler, bool add);

    // Windows argv quoting: double backslashes before quotes and the closing quote.
    static string Quote(string arg) {
        var result = new StringBuilder("\"");
        int slashes = 0;
        foreach (char c in arg) {
            if (c == '\\') { slashes++; continue; }
            result.Append('\\', c == '"' ? slashes * 2 + 1 : slashes);
            result.Append(c);
            slashes = 0;
        }
        result.Append('\\', slashes * 2);
        return result.Append('"').ToString();
    }

    public static int Run(string[] args) {
        IntPtr input = GetStdHandle(-10), output = GetStdHandle(-11);
        uint inputMode, outputMode;
        bool inputTty = GetConsoleMode(input, out inputMode);
        bool outputTty = GetConsoleMode(output, out outputMode);
        // Register last so PowerShell cannot cancel the pipeline before our finally runs.
        ControlHandler cancel = type => type == 0 || type == 1; // Ctrl+C / Ctrl+Break
        SetConsoleCtrlHandler(cancel, true);
        try {
            var line = new StringBuilder("-o ServerAliveInterval=15 -o ServerAliveCountMax=3");
            foreach (string arg in args) line.Append(' ').Append(Quote(arg));
            using (var ssh = new Process()) {
                ssh.StartInfo = new ProcessStartInfo("ssh.exe", line.ToString()) {
                    UseShellExecute = false
                };
                ssh.Start();
                // Ctrl+C also reaches SSH in this console; keep the wrapper alive for cleanup.
                ssh.WaitForExit();
                return ssh.ExitCode;
            }
        } catch (Exception e) {
            Console.Error.WriteLine("ssh-clean: " + e.Message);
            return 127;
        } finally {
            if (inputTty) SetConsoleMode(input, inputMode);
            if (outputTty) {
                SetConsoleMode(output, outputMode | 4); // ENABLE_VIRTUAL_TERMINAL_PROCESSING
                Console.Write("\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1004l\x1b[?2004l\x1b[0m\x1b[?25h");
                Console.Out.Flush();
                SetConsoleMode(output, outputMode);
            }
            SetConsoleCtrlHandler(cancel, false);
        }
    }
}
'@
$status = [TunnelBarSshClean]::Run($SshArgs)
exit $status
