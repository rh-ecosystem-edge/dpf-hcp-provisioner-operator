/*
Copyright 2026.

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
*/

package ignitiongenerator

import (
	"bytes"
	"compress/gzip"
	"encoding/base64"
	"encoding/json"
	"encoding/pem"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"text/template"

	igntypes "github.com/coreos/ignition/v2/config/v3_4/types"
	dpuprovisioningv1alpha1 "github.com/nvidia/doca-platform/api/provisioning/v1alpha1"
	. "github.com/onsi/gomega"

	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/ignition"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/ignition/content"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/ignition/resources/target"
)

const caTrustPath = "/etc/pki/ca-trust/source/anchors/dpf-ca.crt"

// DPF renders the outer bf.cfg using text/template and Sprig's b64enc, whose
// string-to-base64 contract is reproduced here without adding a dependency.
func renderDPFTemplate(raw []byte, bundle string) ([]byte, error) {
	funcs := template.FuncMap{
		"b64enc": func(s string) string { return base64.StdEncoding.EncodeToString([]byte(s)) },
	}
	tmpl, err := template.New("bf.cfg").Funcs(funcs).Parse(string(raw))
	if err != nil {
		return nil, err
	}
	params := map[string]any{
		"CATrustBundle": bundle, "DPUHostName": "bf2-worker-0", "DPUName": "dpu-0",
		"DPUNamespace": "dpf-operator-system", "DPUUID": "12345678-1234-1234-1234-123456789abc",
		"KernelParameters": "console=ttyAMA0", "ControlPlaneMTU": 1500,
		"BootstrapKubeconfig": "", "BFBRegistryURL": "https://registry.example.com:30000",
	}
	var rendered bytes.Buffer
	err = tmpl.Execute(&rendered, params)
	return rendered.Bytes(), err
}

func testCAPEM(t *testing.T) string {
	t.Helper()
	server := httptest.NewTLSServer(http.NotFoundHandler())
	t.Cleanup(server.Close)
	return string(pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: server.Certificate().Raw}))
}

func TestCATrustBundleTemplateGate(t *testing.T) {
	for _, zeroTrust := range []bool{false, true} {
		for _, includeCA := range []bool{false, true} {
			t.Run(fmt.Sprintf("zero-trust=%t/include-ca=%t", zeroTrust, includeCA), func(t *testing.T) {
				g := NewWithT(t)
				ig := &IgnitionGenerator{}
				live, err := ig.buildLiveIgnition(ignition.NewEmptyIgnition(testIgnitionVersion),
					buildRealisticHCPIgnition(), &dpuprovisioningv1alpha1.DPUFlavor{}, zeroTrust, zeroTrust || includeCA, false, includeCA)
				g.Expect(err).NotTo(HaveOccurred())
				var anchors []igntypes.File
				for _, file := range live.Storage.Files {
					if file.Path == caTrustPath {
						anchors = append(anchors, file)
					}
				}
				if includeCA {
					g.Expect(anchors).To(HaveLen(1))
				} else {
					g.Expect(anchors).To(BeEmpty())
				}
				raw, err := ignition.MarshalJSON(live)
				g.Expect(err).NotTo(HaveOccurred())
				g.Expect(strings.Contains(string(raw), "{{ .CATrustBundle | b64enc }}")).To(Equal(includeCA))
			})
		}
	}
}

func TestCATrustBundleRendering(t *testing.T) {
	cert := testCAPEM(t)
	for _, zeroTrust := range []bool{false, true} {
		for name, bundle := range map[string]string{
			"empty": "", "single": cert, "multiple": cert + "\n" + cert,
			"no-final-newline": strings.TrimSuffix(cert+cert, "\n"),
		} {
			t.Run(fmt.Sprintf("zero-trust=%t/%s", zeroTrust, name), func(t *testing.T) {
				g := NewWithT(t)
				ig := &IgnitionGenerator{}
				live, err := ig.buildLiveIgnition(ignition.NewEmptyIgnition(testIgnitionVersion),
					buildRealisticHCPIgnition(), &dpuprovisioningv1alpha1.DPUFlavor{}, zeroTrust, true, false, true)
				g.Expect(err).NotTo(HaveOccurred())
				var anchors []igntypes.File
				for _, file := range live.Storage.Files {
					if file.Path == caTrustPath {
						anchors = append(anchors, file)
					}
				}
				g.Expect(anchors).To(HaveLen(1))
				g.Expect(anchors[0].Mode).To(HaveValue(Equal(0644)))
				g.Expect(anchors[0].Overwrite).To(HaveValue(BeTrue()))
				g.Expect(anchors[0].Contents.Compression).To(BeNil())
				g.Expect(anchors[0].Contents.Source).To(HaveValue(Equal("data:;base64,{{ .CATrustBundle | b64enc }}")))

				raw, err := ignition.MarshalJSON(live)
				g.Expect(err).NotTo(HaveOccurred())
				rendered, err := renderDPFTemplate(raw, bundle)
				g.Expect(err).NotTo(HaveOccurred())
				var resolved igntypes.Config
				g.Expect(json.Unmarshal(rendered, &resolved)).To(Succeed())
				g.Expect(ignition.ValidateConfig(&resolved)).To(Succeed())
				for _, file := range resolved.Storage.Files {
					if file.Path == caTrustPath {
						decoded, err := base64.StdEncoding.DecodeString(strings.TrimPrefix(*file.Contents.Source, "data:;base64,"))
						g.Expect(err).NotTo(HaveOccurred())
						g.Expect(string(decoded)).To(Equal(bundle))
					}
				}
			})
		}
	}
}

func TestCATrustBundleHandoff(t *testing.T) {
	python, err := exec.LookPath("python3")
	if err != nil {
		t.Fatal("python3 is required to test the embedded ignition updater")
	}
	// Run the actual updater, mocking only fixed live filesystem inputs. All
	// other reads/writes are restricted to a disposable directory, never /etc.
	const harness = `
import builtins, io, json, os, runpy, sys
from unittest.mock import patch

script, target, case_path = sys.argv[1:]
with open(case_path) as f:
    case = json.load(f)
real_open = builtins.open
ca_path = '/etc/pki/ca-trust/source/anchors/dpf-ca.crt'
def live_open(path, mode='r', *args, **kwargs):
    if path == ca_path:
        if case['unreadable']:
            raise PermissionError(path)
        if case['bundle'] is None:
            raise FileNotFoundError(path)
        assert mode == 'rb', mode
        return io.BytesIO(case['bundle'].encode())
    if path == '/etc/hostname':
        return io.StringIO('test-dpu\n')
    if path == '/etc/dpf/environment':
        return io.StringIO('DPUName=test-dpu\nDPUMode=zero-trust\n')
    if path == target:
        return real_open(path, mode, *args, **kwargs)
    raise AssertionError('unexpected filesystem access: ' + str(path))
sys.argv = [script, target]
with patch('builtins.open', side_effect=live_open), patch('os.path.exists', return_value=False):
    runpy.run_path(script, run_name='__main__')
`
	cert := testCAPEM(t)
	for _, tc := range []struct {
		name       string
		bundle     *string
		unreadable bool
		existing   bool
	}{
		{name: "missing"}, {name: "empty", bundle: ignition.Ptr("")},
		{name: "whitespace", bundle: ignition.Ptr(" \n\t")},
		{name: "multiple", bundle: ignition.Ptr(cert + "\n" + cert)},
		{name: "replace-owned-anchor", bundle: ignition.Ptr(cert), existing: true},
		{name: "no-final-newline", bundle: ignition.Ptr(strings.TrimSuffix(cert, "\n"))},
		{name: "unreadable", unreadable: true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			g := NewWithT(t)
			ig := &IgnitionGenerator{}
			live, err := ig.buildLiveIgnition(ignition.NewEmptyIgnition(testIgnitionVersion),
				buildRealisticHCPIgnition(), &dpuprovisioningv1alpha1.DPUFlavor{}, false, true, false, true)
			g.Expect(err).NotTo(HaveOccurred())
			var updater igntypes.File
			for _, file := range live.Storage.Files {
				if file.Path == "/usr/local/bin/update_ignition.py" {
					updater = file
				}
			}
			g.Expect(updater.Contents.Source).NotTo(BeNil())
			compressed, err := base64.StdEncoding.DecodeString(strings.TrimPrefix(*updater.Contents.Source, "data:;base64,"))
			g.Expect(err).NotTo(HaveOccurred())
			reader, err := gzip.NewReader(bytes.NewReader(compressed))
			g.Expect(err).NotTo(HaveOccurred())
			script, err := io.ReadAll(reader)
			g.Expect(err).NotTo(HaveOccurred())
			g.Expect(reader.Close()).To(Succeed())
			dir := t.TempDir()
			scriptPath, targetPath, casePath := filepath.Join(dir, "updater.py"), filepath.Join(dir, "target.ign"), filepath.Join(dir, "case.json")
			g.Expect(os.WriteFile(scriptPath, script, 0600)).To(Succeed())
			initial := `{"ignition":{"version":"3.4.0"},"storage":{"files":[` +
				`{"path":"/etc/unrelated","mode":420,"contents":{"source":"data:,keep"}}`
			if tc.existing {
				initial += `,{"path":"` + caTrustPath + `","mode":420,"contents":{"source":"data:,old"}}`
			}
			initial += `]}}`
			g.Expect(os.WriteFile(targetPath, []byte(initial), 0600)).To(Succeed())
			caseJSON, err := json.Marshal(map[string]any{"bundle": tc.bundle, "unreadable": tc.unreadable})
			g.Expect(err).NotTo(HaveOccurred())
			g.Expect(os.WriteFile(casePath, caseJSON, 0600)).To(Succeed())
			output, err := exec.Command(python, "-c", harness, scriptPath, targetPath, casePath).CombinedOutput()
			if tc.unreadable {
				g.Expect(err).To(HaveOccurred(), string(output))
				unchanged, readErr := os.ReadFile(targetPath)
				g.Expect(readErr).NotTo(HaveOccurred())
				g.Expect(string(unchanged)).To(Equal(initial))
				return
			}
			g.Expect(err).NotTo(HaveOccurred(), string(output))
			raw, err := os.ReadFile(targetPath)
			g.Expect(err).NotTo(HaveOccurred())
			var updated igntypes.Config
			g.Expect(json.Unmarshal(raw, &updated)).To(Succeed())
			g.Expect(ignition.ValidateConfig(&updated)).To(Succeed())
			var anchors []igntypes.File
			for _, file := range updated.Storage.Files {
				if file.Path == caTrustPath {
					anchors = append(anchors, file)
				}
			}
			g.Expect(updated.Storage.Files[0].Path).To(Equal("/etc/unrelated"))
			g.Expect(updated.Storage.Files[0].Contents.Source).To(HaveValue(Equal("data:,keep")))
			if tc.bundle == nil || strings.TrimSpace(*tc.bundle) == "" {
				g.Expect(anchors).To(BeEmpty())
				return
			}
			g.Expect(anchors).To(HaveLen(1))
			g.Expect(anchors[0].Mode).To(HaveValue(Equal(0644)))
			g.Expect(anchors[0].Overwrite).To(HaveValue(BeTrue()))
			decoded, err := base64.StdEncoding.DecodeString(strings.TrimPrefix(*anchors[0].Contents.Source, "data:;base64,"))
			g.Expect(err).NotTo(HaveOccurred())
			g.Expect(string(decoded)).To(Equal(*tc.bundle))
		})
	}
}

func TestCAHandoffPreservesInstallerCommands(t *testing.T) {
	for _, zeroTrust := range []bool{false, true} {
		ig := &IgnitionGenerator{}
		live, err := ig.buildLiveIgnition(ignition.NewEmptyIgnition(testIgnitionVersion),
			buildRealisticHCPIgnition(), &dpuprovisioningv1alpha1.DPUFlavor{}, zeroTrust, true, false, true)
		NewWithT(t).Expect(err).NotTo(HaveOccurred())
		targetIgn := ignition.NewEmptyIgnition(testIgnitionVersion)
		NewWithT(t).Expect(content.AddContent(targetIgn, target.NewProvider(zeroTrust))).To(Succeed())
		for name, cfg := range map[string]*igntypes.Config{"install-rhcos-dpf": live, "install-dpu-agent": targetIgn} {
			t.Run(fmt.Sprintf("zero-trust=%t/%s", zeroTrust, name), func(t *testing.T) {
				g := NewWithT(t)
				var unit string
				for _, u := range cfg.Systemd.Units {
					if u.Name == name+".service" {
						unit = *u.Contents
					}
				}
				g.Expect(unit).To(ContainSubstring("ExecStart=/usr/local/bin/" + name + ".sh"))
				// Standard RHCOS refreshes the trust store during boot; the CA
				// handoff must not add a duplicate refresh to the installers.
				g.Expect(unit).NotTo(ContainSubstring("update-ca-trust"))
			})
		}
	}
}
