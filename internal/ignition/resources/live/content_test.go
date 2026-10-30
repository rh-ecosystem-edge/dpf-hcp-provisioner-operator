package live

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func installerScript(t *testing.T) string {
	t.Helper()
	script, err := filesFS.ReadFile("files/install-rhcos-dpf.sh")
	if err != nil {
		t.Fatal(err)
	}
	return string(script)
}

func TestInstallerReservesRshimForCompletion(t *testing.T) {
	script := installerScript(t)
	for _, forbidden := range []string{"/usr/local/bin/bflog.sh", "rsh_log"} {
		t.Run(forbidden, func(t *testing.T) {
			if strings.Contains(script, forbidden) {
				t.Fatalf("installer must not write routine messages through %q", forbidden)
			}
		})
	}

	completion := `if ! is_zero_trust; then
    /usr/local/bin/bfupsignal.sh
    dpu_agent update-time`
	if !strings.Contains(script, completion) {
		t.Fatal("trusted completion signaling and startup reporting must be preserved")
	}

	steps := []string{
		"\ninstall_rhcos\nsetup_RHCOS_EFI_record\nsync\n",
		completion,
		`until [ "$(dpu_agent get-dpu-phase)" = "DPU Config" ]; do`,
		"\n    wait_for_host_reboot_if_required\nfi\n",
		"\nsleep 10\n",
		"\nreboot\n",
	}
	rest := script
	for _, step := range steps {
		_, after, found := strings.Cut(rest, step)
		if !found {
			t.Fatalf("missing or out-of-order installation step: %q", step)
		}
		rest = after
	}
}

func TestInstallerLogRetainsConsoleAndKernelOutput(t *testing.T) {
	script := installerScript(t)
	_, after, found := strings.Cut(script, "\nlog() {\n")
	if !found {
		t.Fatal("installer log function not found")
	}
	body, _, found := strings.Cut(after, "\n}\n")
	if !found {
		t.Fatal("installer log function is not terminated")
	}

	// Redirect only the kernel sink to a temporary file. Never access /dev/kmsg
	// or execute the installation itself in this regression test.
	kernelLog := filepath.Join(t.TempDir(), "kmsg")
	body = strings.ReplaceAll(body, "/dev/kmsg", `"$TEST_KMSG"`)
	cmd := exec.Command("bash", "-c", "log() {\n"+body+"\n}\nlog 'routine installation message'\n")
	cmd.Env = append(os.Environ(), "TEST_KMSG="+kernelLog)
	output, err := cmd.CombinedOutput()
	if err != nil {
		t.Fatalf("log function failed: %v\n%s", err, output)
	}
	kernelOutput, err := os.ReadFile(kernelLog)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(output), "routine installation message") || string(output) != string(kernelOutput) {
		t.Fatalf("console and kernel logs differ: console=%q kernel=%q", output, kernelOutput)
	}
}

func TestInstallerServiceKeepsJournalOutput(t *testing.T) {
	unit, err := systemdFS.ReadFile("systemd/install-rhcos-dpf.service")
	if err != nil {
		t.Fatal(err)
	}
	for _, setting := range []string{"StandardOutput=journal+console", "StandardError=journal+console"} {
		if !strings.Contains(string(unit), setting) {
			t.Errorf("installer service must retain %q", setting)
		}
	}
}

func TestInstallerBashSyntax(t *testing.T) {
	cmd := exec.Command("bash", "-n")
	cmd.Stdin = strings.NewReader(installerScript(t))
	if output, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("invalid installer bash syntax: %v\n%s", err, output)
	}
}
