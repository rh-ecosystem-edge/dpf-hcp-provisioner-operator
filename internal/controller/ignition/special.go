/*
Copyright 2025.

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

package ignition

import (
	"encoding/base64"
	"encoding/json"
	"fmt"
	"net/url"
	"strings"
)

// ReplaceMachineOSURL updates the osImageURL inside the
// /etc/ignition-machine-config-encapsulated.json file stored in the ignition config.
// The file's source is expected to be a URL-encoded JSON string (data:,{urlencoded}).
func ReplaceMachineOSURL(ign *Ignition, machineOSURL string) error {
	const targetPath = "/etc/ignition-machine-config-encapsulated.json"
	for i, f := range ign.Storage.Files {
		if f.Path != targetPath || f.Contents == nil {
			continue
		}
		src := f.Contents.Source
		const prefix = "data:,"
		if !strings.HasPrefix(src, prefix) {
			return fmt.Errorf("unexpected source format for %s: %q", targetPath, src)
		}
		raw, err := url.QueryUnescape(strings.TrimPrefix(src, prefix))
		if err != nil {
			return fmt.Errorf("url-decoding %s: %w", targetPath, err)
		}
		var obj map[string]interface{}
		if err := json.Unmarshal([]byte(raw), &obj); err != nil {
			return fmt.Errorf("unmarshaling %s: %w", targetPath, err)
		}
		spec, _ := obj["spec"].(map[string]interface{})
		if spec == nil {
			spec = make(map[string]interface{})
			obj["spec"] = spec
		}
		spec["osImageURL"] = machineOSURL
		updated, err := json.Marshal(obj)
		if err != nil {
			return fmt.Errorf("marshaling updated %s: %w", targetPath, err)
		}
		ign.Storage.Files[i].Contents.Source = "data:," + url.QueryEscape(string(updated))
		return nil
	}
	return fmt.Errorf("file %s not found in ignition storage", targetPath)
}

// mtu9000ConnectionContent returns an NMConnection file body that sets MTU 9216 on the given interface.
func mtu9000ConnectionContent(ifname string) string {
	return fmt.Sprintf(
		"[connection]\nid=%s\ntype=ethernet\ninterface-name=%s\n\n[ethernet]\nmtu=9216\n\n[ipv4]\nmethod=disabled\n\n[ipv6]\nmethod=ignore\n",
		ifname, ifname,
	)
}

// EnableMTU9000 adds MTU 9216 NMConnection files for p0, p1, pf0hpf, pf1hpf and patches
// any existing pf0vf0.nmconnection in the ignition storage to also carry mtu=9216.
func EnableMTU9000(ign *Ignition) {
	mode := octalToDecimal(600)
	for _, ifname := range []string{"p0", "p1", "pf0hpf", "pf1hpf"} {
		content := mtu9000ConnectionContent(ifname)
		fc := EncodeGzipFile([]byte(content))
		modeCopy := mode
		ign.Storage.Files = append(ign.Storage.Files, FileEntry{
			Path:     "/etc/NetworkManager/system-connections/" + ifname + ".nmconnection",
			Mode:     &modeCopy,
			Contents: &fc,
		})
	}

	// Patch existing pf0vf0.nmconnection to add mtu=9216 under [ethernet].
	const pf0Target = "pf0vf0.nmconnection"
	for i, f := range ign.Storage.Files {
		if !strings.HasSuffix(f.Path, pf0Target) || f.Contents == nil {
			continue
		}
		raw, err := decodeGzipFile(*f.Contents)
		if err != nil {
			break
		}
		content := string(raw)
		if strings.Contains(content, "mtu=") {
			break // already has MTU setting
		}
		content = strings.ReplaceAll(content, "[ethernet]\n", "[ethernet]\nmtu=9216\n")
		fc := EncodeGzipFile([]byte(content))
		ign.Storage.Files[i].Contents = &fc
		break
	}
}

// AddFlavorOVSScript base64-encodes rawConfigScript and adds it as an executable file at
// /usr/local/bin/dpf-ovs-script.sh. No-op when rawConfigScript is empty.
func AddFlavorOVSScript(ign *Ignition, rawConfigScript string) {
	if rawConfigScript == "" {
		return
	}
	encoded := base64.StdEncoding.EncodeToString([]byte(rawConfigScript))
	mode := octalToDecimal(755)
	fc := FileContents{
		Source: "data:;base64," + encoded,
	}
	ign.Storage.Files = append(ign.Storage.Files, FileEntry{
		Path:     "/usr/local/bin/dpf-ovs-script.sh",
		Mode:     &mode,
		Contents: &fc,
	})
}
