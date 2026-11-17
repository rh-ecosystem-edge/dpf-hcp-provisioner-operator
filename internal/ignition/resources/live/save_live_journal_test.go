package live

import (
	"archive/tar"
	"compress/gzip"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

const fakeCommand = `#!/usr/bin/env bash
set -e
printf '%s %s\n' "${0##*/}" "$*" >> "$TEST_EVENTS"
case "${0##*/}" in
    udevadm) ;;
    lsblk)
        [[ "${@: -1}" == /dev/nvme0n1 ]]
        printf '%s' "$TEST_LSBLK_OUTPUT"
        ;;
    mount)
        if [[ "$TEST_MOUNT_FAIL" == true ]]; then exit 1; fi
        ln -s "$TEST_INSTALLED_ROOT/ostree" "${@: -1}/ostree"
        ;;
    journalctl)
        if [[ "$TEST_JOURNAL_FAIL" == true ]]; then exit 1; fi
        printf '%s' "$TEST_JOURNAL_OUTPUT"
        ;;
    tar)
        if [[ "$TEST_TAR_FAIL" == true ]]; then exit 1; fi
        exec "$TEST_REAL_TAR" "$@"
        ;;
    cp)
        if [[ "$TEST_COPY_FAIL" == true ]]; then exit 1; fi
        exec "$TEST_REAL_CP" "$@"
        ;;
esac
`

func TestSaveLiveJournal(t *testing.T) {
	realTar, err := exec.LookPath("tar")
	if err != nil {
		t.Fatal(err)
	}
	realCp, err := exec.LookPath("cp")
	if err != nil {
		t.Fatal(err)
	}

	tests := []struct {
		name             string
		lsblkOutput      string
		mountFail        bool
		missingStateroot bool
		journalFail      bool
		tarFail          bool
		copyFail         bool
		wantSuccess      bool
	}{
		{name: "success", wantSuccess: true},
		{name: "no root partition", lsblkOutput: "/dev/nvme0n1p3 boot\n"},
		{name: "ambiguous root partitions", lsblkOutput: "/dev/nvme0n1p4 root\n/dev/nvme0n1p5 root\n"},
		{name: "mount failure", mountFail: true},
		{name: "missing stateroot", missingStateroot: true},
		{name: "journal failure", journalFail: true},
		{name: "tar failure", tarFail: true},
		{name: "copy failure", copyFail: true},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			base := t.TempDir()
			bin := filepath.Join(base, "bin")
			mountBase := filepath.Join(base, "tmp")
			installedRoot := filepath.Join(base, "installed")
			for _, dir := range []string{bin, mountBase, installedRoot} {
				if err := os.MkdirAll(dir, 0755); err != nil {
					t.Fatal(err)
				}
			}
			if !tt.missingStateroot {
				if err := os.MkdirAll(filepath.Join(installedRoot, "ostree/deploy/rhcos/var"), 0755); err != nil {
					t.Fatal(err)
				}
			}

			wrapper := filepath.Join(bin, "wrapper")
			if err := os.WriteFile(wrapper, []byte(fakeCommand), 0755); err != nil {
				t.Fatal(err)
			}
			for _, name := range []string{"udevadm", "lsblk", "mount", "journalctl", "tar", "cp"} {
				if err := os.Symlink("wrapper", filepath.Join(bin, name)); err != nil {
					t.Fatal(err)
				}
			}

			lsblkOutput := tt.lsblkOutput
			if lsblkOutput == "" {
				lsblkOutput = "/dev/nvme0n1\n/dev/nvme0n1p2 EFI-SYSTEM\n/dev/nvme0n1p4 root\n"
			}
			eventsPath := filepath.Join(base, "events")
			cmd := exec.Command("bash", "files/save-live-journal.sh", "/dev/nvme0n1")
			cmd.Env = append(os.Environ(),
				"COPYFILE_DISABLE=1",
				"PATH="+bin+":"+os.Getenv("PATH"),
				"TMPDIR="+mountBase,
				"TEST_EVENTS="+eventsPath,
				"TEST_INSTALLED_ROOT="+installedRoot,
				"TEST_LSBLK_OUTPUT="+lsblkOutput,
				"TEST_MOUNT_FAIL="+boolString(tt.mountFail),
				"TEST_JOURNAL_FAIL="+boolString(tt.journalFail),
				"TEST_TAR_FAIL="+boolString(tt.tarFail),
				"TEST_COPY_FAIL="+boolString(tt.copyFail),
				"TEST_JOURNAL_OUTPUT=installer started\ncoreos-installer finished\n",
				"TEST_REAL_TAR="+realTar,
				"TEST_REAL_CP="+realCp,
			)
			output, err := cmd.CombinedOutput()
			if (err == nil) != tt.wantSuccess {
				t.Fatalf("unexpected result: %v\n%s", err, output)
			}
			if strings.Contains(string(output), "Saved live boot journal") != tt.wantSuccess {
				t.Fatalf("unexpected success message: %v\n%s", err, output)
			}

			events, err := os.ReadFile(eventsPath)
			if err != nil {
				t.Fatal(err)
			}
			if tt.wantSuccess {
				if !strings.Contains(string(events), "mount -o rw /dev/nvme0n1p4 ") ||
					!strings.Contains(string(events), "journalctl -b --no-pager --output=short-iso-precise") {
					t.Fatalf("wrong disk or journal command:\n%s", events)
				}
			}
			stageDirs, err := filepath.Glob(filepath.Join(mountBase, "dpf-live-journal.*"))
			if err != nil {
				t.Fatal(err)
			}
			if tt.wantSuccess {
				if len(stageDirs) != 1 {
					t.Fatalf("expected archive staging on live ISO: %v", stageDirs)
				}
				if !strings.Contains(string(events), "tar -czf "+filepath.Join(stageDirs[0], "dpu-live-install-journal.tar.gz")) ||
					!strings.Contains(string(events), "cp "+filepath.Join(stageDirs[0], "dpu-live-install-journal.tar.gz")+" ") {
					t.Fatalf("archive was not staged on the live ISO:\n%s", events)
				}
			}
			logDir := filepath.Join(installedRoot, "ostree/deploy/rhcos/var/log")
			archivePath := filepath.Join(logDir, "dpu-live-install-journal.tar.gz")
			archiveInfo, err := os.Stat(archivePath)
			if !tt.wantSuccess {
				if !os.IsNotExist(err) {
					t.Fatalf("unexpected archive on failure: %v", err)
				}
			} else {
				if err != nil {
					t.Fatal(err)
				}
				if archiveInfo.Mode().Perm() != 0600 {
					t.Fatalf("archive permissions = %o, want 600", archiveInfo.Mode().Perm())
				}
				if got := readJournalArchive(t, archivePath); got != "installer started\ncoreos-installer finished\n" {
					t.Fatalf("archived journal = %q", got)
				}
			}

			entries, err := os.ReadDir(logDir)
			if err == nil {
				for _, entry := range entries {
					if strings.HasPrefix(entry.Name(), ".dpu-live-") {
						t.Fatalf("temporary file left in installed /var/log: %s", entry.Name())
					}
				}
			} else if !os.IsNotExist(err) {
				t.Fatal(err)
			}
		})
	}
}

func boolString(value bool) string {
	if value {
		return "true"
	}
	return "false"
}

func readJournalArchive(t *testing.T, archivePath string) string {
	t.Helper()
	file, err := os.Open(archivePath)
	if err != nil {
		t.Fatal(err)
	}
	defer file.Close()
	gz, err := gzip.NewReader(file)
	if err != nil {
		t.Fatal(err)
	}
	defer gz.Close()
	tarReader := tar.NewReader(gz)
	header, err := tarReader.Next()
	if err != nil || header.Name != "journalctl.log" {
		t.Fatalf("archive member = %v, %v", header, err)
	}
	contents, err := io.ReadAll(tarReader)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := tarReader.Next(); err != io.EOF {
		t.Fatalf("unexpected extra archive member: %v", err)
	}
	return string(contents)
}

func TestLiveInstallerJournalWiring(t *testing.T) {
	for _, zeroTrust := range []bool{false, true} {
		found := false
		for _, file := range NewProvider(zeroTrust).GetFiles() {
			if file.Path == "/usr/local/bin/save-live-journal.sh" {
				found = true
				if file.Mode != 0755 || len(file.ContentSource.([]byte)) == 0 {
					t.Fatalf("invalid journal helper for zeroTrust=%t: %+v", zeroTrust, file)
				}
			}
		}
		if !found {
			t.Fatalf("journal helper missing for zeroTrust=%t", zeroTrust)
		}
	}

	script, err := filesFS.ReadFile("files/install-rhcos-dpf.sh")
	if err != nil {
		t.Fatal(err)
	}
	text := string(script)
	install := strings.LastIndex(text, "\ninstall_rhcos\n")
	efiSetup := strings.LastIndex(text, "\nsetup_RHCOS_EFI_record\n")
	save := strings.LastIndex(text, "\nif ! /usr/local/bin/save-live-journal.sh")
	hostSignal := strings.LastIndex(text, "\n    /usr/local/bin/bfupsignal.sh")
	phaseWait := strings.LastIndex(text, `until [ "$(dpu_agent get-dpu-phase)" = "DPU Config" ]; do`)
	hostReboot := strings.LastIndex(text, "\n    wait_for_host_reboot_if_required\n")
	localReboot := strings.LastIndex(text, "\nreboot\n")
	if install < 0 || install >= efiSetup || efiSetup >= hostSignal || hostSignal >= phaseWait ||
		phaseWait >= save || save >= hostReboot || hostReboot >= localReboot {
		t.Fatal("journal helper must run after install signaling and phase wait, but before host power-cycle or local reboot")
	}
}
