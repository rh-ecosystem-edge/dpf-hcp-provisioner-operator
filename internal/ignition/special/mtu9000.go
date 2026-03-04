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

package special

import (
	"fmt"
	"net/url"
	"strings"

	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/ignition"
)

// URLEncode encodes a string to a data URI
func URLEncode(s string) string {
	return "data:text/plain," + url.QueryEscape(s)
}

// nmInterface generates a NetworkManager interface configuration with MTU
func nmInterface(name string, mtu int) string { //nolint:unparam // mtu will be dynamic once NVIDIA-592 is implemented
	config := fmt.Sprintf(`[connection]
id=%s
type=ethernet
interface-name=%s

[ethernet]
mtu=%d
`, name, name, mtu)

	return URLEncode(config)
}

// EnableMTU9000 adds MTU 9000 configuration files to the ignition
func EnableMTU9000(ign *ignition.Ignition) {
	// Add new network manager connections
	newFiles := []ignition.FileEntry{
		{
			Path:      "/etc/NetworkManager/system-connections/p0.nmconnection",
			Overwrite: true,
			Mode:      0600,
			Contents: ignition.FileContents{
				Source: nmInterface("p0", 9216),
			},
		},
		{
			Path:      "/etc/NetworkManager/system-connections/p1.nmconnection",
			Overwrite: true,
			Mode:      0600,
			Contents: ignition.FileContents{
				Source: nmInterface("p1", 9216),
			},
		},
		{
			Path:      "/etc/NetworkManager/system-connections/pf0hpf.nmconnection",
			Overwrite: true,
			Mode:      0600,
			Contents: ignition.FileContents{
				Source: nmInterface("pf0hpf", 9216),
			},
		},
		{
			Path:      "/etc/NetworkManager/system-connections/pf1hpf.nmconnection",
			Overwrite: true,
			Mode:      0600,
			Contents: ignition.FileContents{
				Source: nmInterface("pf1hpf", 9216),
			},
		},
	}

	ign.Storage.Files = append(ign.Storage.Files, newFiles...)

	// Modify existing pf0vf0.nmconnection to add MTU
	for i := range ign.Storage.Files {
		file := &ign.Storage.Files[i]
		if strings.HasSuffix(file.Path, "/etc/NetworkManager/system-connections/pf0vf0.nmconnection") {
			if file.Contents.Source != "" {
				file.Contents.Source = strings.Replace(
					file.Contents.Source,
					"[ethernet]\n",
					"[ethernet]\nmtu=9216\n",
					1,
				)
			}
		}
	}
}
