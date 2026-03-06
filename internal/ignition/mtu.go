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
	"fmt"
	"net/url"
	"strings"

	igntypes "github.com/coreos/ignition/v2/config/v3_4/types"
)

// URLEncode encodes a string to a data URI
func URLEncode(s string) string {
	return "data:text/plain," + url.QueryEscape(s)
}

// nmInterface generates a NetworkManager interface configuration with MTU
func nmInterface(name string, mtu uint16) string {
	config := fmt.Sprintf(`[connection]
id=%s
type=ethernet
interface-name=%s

[ethernet]
mtu=%d
`, name, name, mtu)

	return URLEncode(config)
}

// EnableMTU adds MTU configuration files to the ignition
func EnableMTU(ign *igntypes.Config, mtu uint16) {
	interfaces := []string{"p0", "p1", "pf0hpf", "pf1hpf"}

	for _, iface := range interfaces {
		source := nmInterface(iface, mtu)
		file := igntypes.File{
			Node: igntypes.Node{
				Path:      fmt.Sprintf("/etc/NetworkManager/system-connections/%s.nmconnection", iface),
				Overwrite: Ptr(true),
			},
			FileEmbedded1: igntypes.FileEmbedded1{
				Mode: Ptr(0600),
				Contents: igntypes.Resource{
					Source: &source,
				},
			},
		}
		ign.Storage.Files = append(ign.Storage.Files, file)
	}

	// Modify existing pf0vf0.nmconnection to add MTU
	for i := range ign.Storage.Files {
		file := &ign.Storage.Files[i]
		if strings.HasSuffix(file.Path, "/etc/NetworkManager/system-connections/pf0vf0.nmconnection") {
			if file.Contents.Source != nil && *file.Contents.Source != "" {
				modified := strings.Replace(
					*file.Contents.Source,
					"[ethernet]\n",
					fmt.Sprintf("[ethernet]\nmtu=%d\n", mtu),
					1,
				)
				file.Contents.Source = &modified
			}
		}
	}
}
