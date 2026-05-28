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
	"bytes"
	"compress/gzip"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"net/url"
	"strings"
	"testing"

	igntypes "github.com/coreos/ignition/v2/config/v3_4/types"
	dpuprovisioningv1alpha1 "github.com/nvidia/doca-platform/api/provisioning/v1alpha1"
	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"sigs.k8s.io/yaml"
)

const compressionGzip = "gzip"

const testIgnitionVersion = "3.4.0"

func TestIgnition(t *testing.T) {
	RegisterFailHandler(Fail)
	RunSpecs(t, "Ignition Suite")
}

var _ = Describe("NewEmptyIgnition", func() {
	It("should create ignition with the specified version", func() {
		ign := NewEmptyIgnition(testIgnitionVersion)
		Expect(ign.Ignition.Version).To(Equal(testIgnitionVersion))
	})

	It("should default to IgnitionVersion when version is empty", func() {
		ign := NewEmptyIgnition("")
		Expect(ign.Ignition.Version).To(Equal(IgnitionVersion))
	})
})

var _ = Describe("MarshalJSON", func() {
	It("should strip empty objects from JSON output", func() {
		ign := NewEmptyIgnition("3.4.0")
		data, err := MarshalJSON(ign)
		Expect(err).NotTo(HaveOccurred())

		var m map[string]any
		Expect(json.Unmarshal(data, &m)).To(Succeed())

		// Should have ignition version but no empty storage/systemd/passwd
		Expect(m).To(HaveKey("ignition"))
		Expect(m).NotTo(HaveKey("storage"))
		Expect(m).NotTo(HaveKey("systemd"))
		Expect(m).NotTo(HaveKey("passwd"))
	})

	It("should preserve non-empty data", func() {
		ign := NewEmptyIgnition("3.4.0")
		source := "data:,hello"
		ign.Storage.Files = append(ign.Storage.Files, igntypes.File{
			Node:          igntypes.Node{Path: "/etc/test"},
			FileEmbedded1: igntypes.FileEmbedded1{Contents: igntypes.Resource{Source: &source}},
		})

		data, err := MarshalJSON(ign)
		Expect(err).NotTo(HaveOccurred())

		var m map[string]any
		Expect(json.Unmarshal(data, &m)).To(Succeed())
		Expect(m).To(HaveKey("storage"))
	})
})

var _ = Describe("EncodeIgnition / DecodeIgnition", func() {
	It("should round-trip encode and decode an ignition config", func() {
		ign := NewEmptyIgnition("3.4.0")
		source := "data:,test-content"
		ign.Storage.Files = append(ign.Storage.Files, igntypes.File{
			Node:          igntypes.Node{Path: "/etc/test"},
			FileEmbedded1: igntypes.FileEmbedded1{Contents: igntypes.Resource{Source: &source}},
		})

		encoded, err := EncodeIgnition(ign)
		Expect(err).NotTo(HaveOccurred())
		Expect(encoded).NotTo(BeEmpty())

		decoded, err := DecodeIgnition(encoded)
		Expect(err).NotTo(HaveOccurred())
		Expect(decoded.Ignition.Version).To(Equal("3.4.0"))
		Expect(decoded.Storage.Files).To(HaveLen(1))
		Expect(decoded.Storage.Files[0].Path).To(Equal("/etc/test"))
	})

	It("should fail to decode invalid base64", func() {
		_, err := DecodeIgnition("not-valid-base64!!!")
		Expect(err).To(HaveOccurred())
		Expect(err.Error()).To(ContainSubstring("base64"))
	})

	It("should fail to decode invalid gzip", func() {
		notGzip := base64.StdEncoding.EncodeToString([]byte("this is not gzip"))
		_, err := DecodeIgnition(notGzip)
		Expect(err).To(HaveOccurred())
		Expect(err.Error()).To(ContainSubstring("gzip"))
	})
})

var _ = Describe("EncodeGzipFile", func() {
	It("should return a gzip-compressed base64-encoded Resource", func() {
		content := []byte("#!/bin/bash\necho hello")
		resource, err := EncodeGzipFile(content)
		Expect(err).NotTo(HaveOccurred())
		Expect(resource.Source).NotTo(BeNil())
		Expect(*resource.Source).To(HavePrefix("data:;base64,"))
		Expect(resource.Compression).NotTo(BeNil())
		Expect(*resource.Compression).To(Equal(compressionGzip))
	})

	It("should produce decompressible content that matches the input", func() {
		content := []byte("test file content for round-trip")
		resource, err := EncodeGzipFile(content)
		Expect(err).NotTo(HaveOccurred())

		// Extract base64 data
		b64 := strings.TrimPrefix(*resource.Source, "data:;base64,")
		compressed, err := base64.StdEncoding.DecodeString(b64)
		Expect(err).NotTo(HaveOccurred())

		// Decompress
		gzReader, err := gzip.NewReader(bytes.NewReader(compressed))
		Expect(err).NotTo(HaveOccurred())
		defer gzReader.Close()

		var buf bytes.Buffer
		_, err = buf.ReadFrom(gzReader)
		Expect(err).NotTo(HaveOccurred())
		Expect(buf.Bytes()).To(Equal(content))
	})
})

var _ = Describe("ReplaceMachineOSURL", func() {
	// Helper to build an ignition config with the machine config encapsulated file
	buildIgnWithMachineConfig := func(osImageURL string) *igntypes.Config {
		machineConfig := map[string]interface{}{
			"spec": map[string]interface{}{
				"osImageURL": osImageURL,
			},
		}
		jsonBytes, err := json.Marshal(machineConfig)
		Expect(err).NotTo(HaveOccurred())

		encoded := url.QueryEscape(string(jsonBytes))
		source := fmt.Sprintf("data:,%s", encoded)

		ign := NewEmptyIgnition("3.4.0")
		ign.Storage.Files = append(ign.Storage.Files, igntypes.File{
			Node:          igntypes.Node{Path: "/etc/ignition-machine-config-encapsulated.json"},
			FileEmbedded1: igntypes.FileEmbedded1{Contents: igntypes.Resource{Source: &source}},
		})
		return ign
	}

	It("should replace the osImageURL in the machine config", func() {
		ign := buildIgnWithMachineConfig("https://old-url.example.com/image")
		err := ReplaceMachineOSURL(ign, "https://new-url.example.com/image")
		Expect(err).NotTo(HaveOccurred())

		// Decode and verify
		source := *ign.Storage.Files[0].Contents.Source
		encodedData := strings.TrimPrefix(source, "data:,")
		decoded, err := url.QueryUnescape(encodedData)
		Expect(err).NotTo(HaveOccurred())

		var config map[string]interface{}
		Expect(json.Unmarshal([]byte(decoded), &config)).To(Succeed())
		spec := config["spec"].(map[string]interface{})
		Expect(spec["osImageURL"]).To(Equal("https://new-url.example.com/image"))
	})

	It("should return error when the file is not found", func() {
		ign := NewEmptyIgnition("3.4.0") // no files
		err := ReplaceMachineOSURL(ign, "https://example.com")
		Expect(err).To(HaveOccurred())
		Expect(err.Error()).To(ContainSubstring("machine OS URL file not found"))
	})

	It("should return error when the data URI format is unexpected", func() {
		ign := NewEmptyIgnition("3.4.0")
		badSource := "not-a-data-uri"
		ign.Storage.Files = append(ign.Storage.Files, igntypes.File{
			Node:          igntypes.Node{Path: "/etc/ignition-machine-config-encapsulated.json"},
			FileEmbedded1: igntypes.FileEmbedded1{Contents: igntypes.Resource{Source: &badSource}},
		})

		err := ReplaceMachineOSURL(ign, "https://example.com")
		Expect(err).To(HaveOccurred())
		Expect(err.Error()).To(ContainSubstring("unexpected data URI format"))
	})

	It("should return error when data URI contains invalid JSON", func() {
		source := "data:," + url.QueryEscape("not-json")
		ign := NewEmptyIgnition("3.4.0")
		ign.Storage.Files = append(ign.Storage.Files, igntypes.File{
			Node:          igntypes.Node{Path: "/etc/ignition-machine-config-encapsulated.json"},
			FileEmbedded1: igntypes.FileEmbedded1{Contents: igntypes.Resource{Source: &source}},
		})

		err := ReplaceMachineOSURL(ign, "https://example.com")
		Expect(err).To(HaveOccurred())
		Expect(err.Error()).To(ContainSubstring("failed to unmarshal"))
	})
})

var _ = Describe("AddFlavorOVSScript", func() {
	It("should add the OVS script file to ignition", func() {
		ign := NewEmptyIgnition("3.4.0")
		flavorSpec := &dpuprovisioningv1alpha1.DPUFlavorSpec{
			OVS: dpuprovisioningv1alpha1.DPUFlavorOVS{RawConfigScript: "#!/bin/bash\novs-vsctl add-br br0"},
		}

		AddFlavorOVSScript(ign, flavorSpec)

		Expect(ign.Storage.Files).To(HaveLen(1))
		file := ign.Storage.Files[0]
		Expect(file.Path).To(Equal("/usr/local/bin/dpf-ovs-script.sh"))
		Expect(*file.Mode).To(Equal(0755))
		Expect(*file.Overwrite).To(BeTrue())
	})

	It("should base64-encode the script content", func() {
		ign := NewEmptyIgnition("3.4.0")
		script := "#!/bin/bash\necho test"
		flavorSpec := &dpuprovisioningv1alpha1.DPUFlavorSpec{OVS: dpuprovisioningv1alpha1.DPUFlavorOVS{RawConfigScript: script}}

		AddFlavorOVSScript(ign, flavorSpec)

		source := *ign.Storage.Files[0].Contents.Source
		Expect(source).To(HavePrefix("data:text/plain;charset=utf-8;base64,"))
		b64 := strings.TrimPrefix(source, "data:text/plain;charset=utf-8;base64,")
		decoded, err := base64.StdEncoding.DecodeString(b64)
		Expect(err).NotTo(HaveOccurred())
		Expect(string(decoded)).To(Equal(script))
	})

	It("should handle an empty script", func() {
		ign := NewEmptyIgnition("3.4.0")
		flavorSpec := &dpuprovisioningv1alpha1.DPUFlavorSpec{OVS: dpuprovisioningv1alpha1.DPUFlavorOVS{RawConfigScript: ""}}

		AddFlavorOVSScript(ign, flavorSpec)
		Expect(ign.Storage.Files).To(HaveLen(1))
	})
})

var _ = Describe("EnableMTU", func() {
	It("should add NM connection files for p0, p1, pf0hpf, pf1hpf", func() {
		ign := NewEmptyIgnition("3.4.0")
		EnableMTU(ign, 9000)

		expectedInterfaces := []string{"p0", "p1", "pf0hpf", "pf1hpf"}
		Expect(ign.Storage.Files).To(HaveLen(len(expectedInterfaces)))

		for i, iface := range expectedInterfaces {
			expectedPath := fmt.Sprintf("/etc/NetworkManager/system-connections/%s.nmconnection", iface)
			Expect(ign.Storage.Files[i].Path).To(Equal(expectedPath))
		}
	})

	It("should set correct MTU value in the generated config", func() {
		ign := NewEmptyIgnition("3.4.0")
		EnableMTU(ign, 9000)

		source := *ign.Storage.Files[0].Contents.Source
		decoded, err := url.QueryUnescape(strings.TrimPrefix(source, "data:text/plain,"))
		Expect(err).NotTo(HaveOccurred())
		Expect(decoded).To(ContainSubstring("mtu=9000"))
	})

	It("should set file mode 0600 for NM connection files", func() {
		ign := NewEmptyIgnition("3.4.0")
		EnableMTU(ign, 9000)

		for _, file := range ign.Storage.Files {
			Expect(*file.Mode).To(Equal(0600))
		}
	})

	It("should modify existing pf0vf0.nmconnection to add MTU", func() {
		ign := NewEmptyIgnition("3.4.0")
		existingSource := "data:text/plain,[connection]\nid=pf0vf0\n\n[ethernet]\nwake-on-lan=0\n"
		ign.Storage.Files = append(ign.Storage.Files, igntypes.File{
			Node:          igntypes.Node{Path: "/etc/NetworkManager/system-connections/pf0vf0.nmconnection"},
			FileEmbedded1: igntypes.FileEmbedded1{Contents: igntypes.Resource{Source: &existingSource}},
		})

		EnableMTU(ign, 9000)

		// Find the pf0vf0 file (should be the first one, before the 4 new ones)
		var pf0vf0Source string
		for _, file := range ign.Storage.Files {
			if strings.HasSuffix(file.Path, "pf0vf0.nmconnection") {
				pf0vf0Source = *file.Contents.Source
				break
			}
		}
		Expect(pf0vf0Source).To(ContainSubstring("mtu=9000"))
	})
})

var _ = Describe("GzipIgnitionFiles", func() {
	It("should compress data URI files", func() {
		ign := NewEmptyIgnition("3.4.0")
		source := "data:,hello+world"
		ign.Storage.Files = append(ign.Storage.Files, igntypes.File{
			Node:          igntypes.Node{Path: "/etc/test"},
			FileEmbedded1: igntypes.FileEmbedded1{Contents: igntypes.Resource{Source: &source}},
		})

		Expect(GzipIgnitionFiles(ign)).To(Succeed())

		file := ign.Storage.Files[0]
		Expect(file.Contents.Compression).NotTo(BeNil())
		Expect(*file.Contents.Compression).To(Equal(compressionGzip))
		Expect(*file.Contents.Source).To(HavePrefix("data:;base64,"))
	})

	It("should compress base64 files", func() {
		ign := NewEmptyIgnition("3.4.0")
		encoded := base64.StdEncoding.EncodeToString([]byte("test content"))
		source := fmt.Sprintf("data:;base64,%s", encoded)
		ign.Storage.Files = append(ign.Storage.Files, igntypes.File{
			Node:          igntypes.Node{Path: "/etc/test"},
			FileEmbedded1: igntypes.FileEmbedded1{Contents: igntypes.Resource{Source: &source}},
		})

		Expect(GzipIgnitionFiles(ign)).To(Succeed())

		file := ign.Storage.Files[0]
		Expect(*file.Contents.Compression).To(Equal(compressionGzip))
	})

	It("should skip already compressed files", func() {
		ign := NewEmptyIgnition("3.4.0")
		source := "data:;base64,already-compressed"
		compression := compressionGzip
		ign.Storage.Files = append(ign.Storage.Files, igntypes.File{
			Node: igntypes.Node{Path: "/etc/test"},
			FileEmbedded1: igntypes.FileEmbedded1{Contents: igntypes.Resource{
				Source:      &source,
				Compression: &compression,
			}},
		})

		Expect(GzipIgnitionFiles(ign)).To(Succeed())

		// Source should be unchanged
		Expect(*ign.Storage.Files[0].Contents.Source).To(Equal(source))
	})

	It("should skip files with nil or empty source", func() {
		ign := NewEmptyIgnition("3.4.0")
		ign.Storage.Files = append(ign.Storage.Files, igntypes.File{
			Node:          igntypes.Node{Path: "/etc/test"},
			FileEmbedded1: igntypes.FileEmbedded1{Contents: igntypes.Resource{}},
		})

		Expect(GzipIgnitionFiles(ign)).To(Succeed())
		Expect(ign.Storage.Files[0].Contents.Source).To(BeNil())
	})

	It("should produce content that decompresses to the original", func() {
		ign := NewEmptyIgnition("3.4.0")
		originalContent := "original file content here"
		source := "data:," + url.QueryEscape(originalContent)
		ign.Storage.Files = append(ign.Storage.Files, igntypes.File{
			Node:          igntypes.Node{Path: "/etc/test"},
			FileEmbedded1: igntypes.FileEmbedded1{Contents: igntypes.Resource{Source: &source}},
		})

		Expect(GzipIgnitionFiles(ign)).To(Succeed())

		// Decompress and verify
		b64 := strings.TrimPrefix(*ign.Storage.Files[0].Contents.Source, "data:;base64,")
		compressed, err := base64.StdEncoding.DecodeString(b64)
		Expect(err).NotTo(HaveOccurred())

		gzReader, err := gzip.NewReader(bytes.NewReader(compressed))
		Expect(err).NotTo(HaveOccurred())
		defer gzReader.Close()

		var buf bytes.Buffer
		_, err = buf.ReadFrom(gzReader)
		Expect(err).NotTo(HaveOccurred())
		Expect(buf.String()).To(Equal(originalContent))
	})
})

var _ = Describe("cleanDPUFlavor", func() {
	It("should keep only TypeMeta, name, namespace, and spec", func() {
		flavor := &dpuprovisioningv1alpha1.DPUFlavor{
			TypeMeta: metav1.TypeMeta{
				APIVersion: "provisioning.dpu.nvidia.com/v1alpha1",
				Kind:       "DPUFlavor",
			},
			ObjectMeta: metav1.ObjectMeta{
				Name:            "test-flavor",
				Namespace:       "test-ns",
				UID:             "some-uid",
				ResourceVersion: "12345",
				Labels:          map[string]string{"key": "value"},
				Annotations:     map[string]string{"ann": "val"},
			},
			Spec: dpuprovisioningv1alpha1.DPUFlavorSpec{
				OVS: dpuprovisioningv1alpha1.DPUFlavorOVS{RawConfigScript: "#!/bin/bash"},
			},
		}

		cleaned := cleanDPUFlavor(flavor)

		Expect(cleaned.TypeMeta).To(Equal(flavor.TypeMeta))
		Expect(cleaned.Name).To(Equal("test-flavor"))
		Expect(cleaned.Namespace).To(Equal("test-ns"))
		Expect(cleaned.Spec).To(Equal(flavor.Spec))
		Expect(cleaned.UID).To(BeEmpty())
		Expect(cleaned.ResourceVersion).To(BeEmpty())
		Expect(cleaned.Labels).To(BeNil())
		Expect(cleaned.Annotations).To(BeNil())
	})
})

var _ = Describe("AddDPUFlavorYAML", func() {
	It("should add a YAML file at the correct path", func() {
		ign := NewEmptyIgnition(testIgnitionVersion)
		flavor := &dpuprovisioningv1alpha1.DPUFlavor{
			ObjectMeta: metav1.ObjectMeta{Name: "my-flavor", Namespace: "ns"},
			Spec: dpuprovisioningv1alpha1.DPUFlavorSpec{
				OVS: dpuprovisioningv1alpha1.DPUFlavorOVS{RawConfigScript: "#!/bin/bash"},
			},
		}

		err := AddDPUFlavorYAML(ign, flavor)
		Expect(err).NotTo(HaveOccurred())
		Expect(ign.Storage.Files).To(HaveLen(1))
		Expect(ign.Storage.Files[0].Path).To(Equal(dpuFlavorYAMLPath))
		Expect(*ign.Storage.Files[0].Mode).To(Equal(0644))
		Expect(*ign.Storage.Files[0].Overwrite).To(BeTrue())
	})

	It("should produce valid YAML content with only cleaned fields", func() {
		ign := NewEmptyIgnition(testIgnitionVersion)
		flavor := &dpuprovisioningv1alpha1.DPUFlavor{
			ObjectMeta: metav1.ObjectMeta{
				Name:            "my-flavor",
				Namespace:       "ns",
				ResourceVersion: "999",
			},
			Spec: dpuprovisioningv1alpha1.DPUFlavorSpec{
				OVS: dpuprovisioningv1alpha1.DPUFlavorOVS{RawConfigScript: "#!/bin/bash"},
			},
		}

		err := AddDPUFlavorYAML(ign, flavor)
		Expect(err).NotTo(HaveOccurred())

		source := *ign.Storage.Files[0].Contents.Source
		b64 := strings.TrimPrefix(source, "data:text/plain;charset=utf-8;base64,")
		decoded, err := base64.StdEncoding.DecodeString(b64)
		Expect(err).NotTo(HaveOccurred())

		var result map[string]interface{}
		err = yaml.Unmarshal(decoded, &result)
		Expect(err).NotTo(HaveOccurred())

		metadata := result["metadata"].(map[string]interface{})
		Expect(metadata["name"]).To(Equal("my-flavor"))
		Expect(metadata["namespace"]).To(Equal("ns"))
		Expect(metadata).NotTo(HaveKey("resourceVersion"))
	})
})

var _ = Describe("AddDPUFlavorJSON", func() {
	It("should add a JSON file at the correct path", func() {
		ign := NewEmptyIgnition(testIgnitionVersion)
		flavor := &dpuprovisioningv1alpha1.DPUFlavor{
			ObjectMeta: metav1.ObjectMeta{Name: "my-flavor", Namespace: "ns"},
			Spec: dpuprovisioningv1alpha1.DPUFlavorSpec{
				OVS: dpuprovisioningv1alpha1.DPUFlavorOVS{RawConfigScript: "#!/bin/bash"},
			},
		}

		err := AddDPUFlavorJSON(ign, flavor)
		Expect(err).NotTo(HaveOccurred())
		Expect(ign.Storage.Files).To(HaveLen(1))
		Expect(ign.Storage.Files[0].Path).To(Equal(dpuFlavorJSONPath))
		Expect(*ign.Storage.Files[0].Mode).To(Equal(0644))
		Expect(*ign.Storage.Files[0].Overwrite).To(BeTrue())
	})

	It("should produce valid JSON content with only cleaned fields", func() {
		ign := NewEmptyIgnition(testIgnitionVersion)
		flavor := &dpuprovisioningv1alpha1.DPUFlavor{
			ObjectMeta: metav1.ObjectMeta{
				Name:            "my-flavor",
				Namespace:       "ns",
				ResourceVersion: "999",
			},
			Spec: dpuprovisioningv1alpha1.DPUFlavorSpec{
				OVS: dpuprovisioningv1alpha1.DPUFlavorOVS{RawConfigScript: "#!/bin/bash"},
			},
		}

		err := AddDPUFlavorJSON(ign, flavor)
		Expect(err).NotTo(HaveOccurred())

		source := *ign.Storage.Files[0].Contents.Source
		b64 := strings.TrimPrefix(source, "data:text/plain;charset=utf-8;base64,")
		decoded, err := base64.StdEncoding.DecodeString(b64)
		Expect(err).NotTo(HaveOccurred())

		var result map[string]interface{}
		err = json.Unmarshal(decoded, &result)
		Expect(err).NotTo(HaveOccurred())

		metadata := result["metadata"].(map[string]interface{})
		Expect(metadata["name"]).To(Equal("my-flavor"))
		Expect(metadata["namespace"]).To(Equal("ns"))
		Expect(metadata).NotTo(HaveKey("resourceVersion"))
	})
})

var _ = Describe("AddFlavorConfigFiles", func() {
	It("should add override files with Overwrite=true and Contents", func() {
		ign := NewEmptyIgnition(testIgnitionVersion)
		flavorSpec := &dpuprovisioningv1alpha1.DPUFlavorSpec{
			ConfigFiles: []dpuprovisioningv1alpha1.ConfigFile{
				{
					Path:        "/etc/mellanox/mlnx-bf.conf",
					Operation:   dpuprovisioningv1alpha1.FileOverride,
					Raw:         "ALLOW_SHARED_RQ=\"no\"\nENABLE_ESWITCH_MULTIPORT=\"yes\"\n",
					Permissions: "0644",
				},
			},
		}

		err := AddFlavorConfigFiles(ign, flavorSpec)
		Expect(err).NotTo(HaveOccurred())
		Expect(ign.Storage.Files).To(HaveLen(1))

		file := ign.Storage.Files[0]
		Expect(file.Path).To(Equal("/etc/mellanox/mlnx-bf.conf"))
		Expect(*file.Overwrite).To(BeTrue())
		Expect(*file.Mode).To(Equal(0644))
		Expect(file.Contents.Source).NotTo(BeNil())
		Expect(file.Append).To(BeEmpty())
	})

	It("should add append files using the Append field", func() {
		ign := NewEmptyIgnition(testIgnitionVersion)
		flavorSpec := &dpuprovisioningv1alpha1.DPUFlavorSpec{
			ConfigFiles: []dpuprovisioningv1alpha1.ConfigFile{
				{
					Path:        "/etc/sysctl.d/99-custom.conf",
					Operation:   dpuprovisioningv1alpha1.FileAppend,
					Raw:         "net.ipv4.ip_forward=1\n",
					Permissions: "0644",
				},
			},
		}

		err := AddFlavorConfigFiles(ign, flavorSpec)
		Expect(err).NotTo(HaveOccurred())
		Expect(ign.Storage.Files).To(HaveLen(1))

		file := ign.Storage.Files[0]
		Expect(file.Path).To(Equal("/etc/sysctl.d/99-custom.conf"))
		Expect(file.Overwrite).To(BeNil())
		Expect(*file.Mode).To(Equal(0644))
		Expect(file.Append).To(HaveLen(1))
		Expect(file.Append[0].Source).NotTo(BeNil())
	})

	It("should base64-encode the raw content correctly", func() {
		ign := NewEmptyIgnition(testIgnitionVersion)
		rawContent := "CREATE_OVS_BRIDGES=\"no\"\nOVS_DOCA=\"yes\"\n"
		flavorSpec := &dpuprovisioningv1alpha1.DPUFlavorSpec{
			ConfigFiles: []dpuprovisioningv1alpha1.ConfigFile{
				{
					Path:        "/etc/mellanox/mlnx-ovs.conf",
					Operation:   dpuprovisioningv1alpha1.FileOverride,
					Raw:         rawContent,
					Permissions: "0644",
				},
			},
		}

		err := AddFlavorConfigFiles(ign, flavorSpec)
		Expect(err).NotTo(HaveOccurred())

		source := *ign.Storage.Files[0].Contents.Source
		Expect(source).To(HavePrefix("data:text/plain;charset=utf-8;base64,"))
		b64 := strings.TrimPrefix(source, "data:text/plain;charset=utf-8;base64,")
		decoded, err := base64.StdEncoding.DecodeString(b64)
		Expect(err).NotTo(HaveOccurred())
		Expect(string(decoded)).To(Equal(rawContent))
	})

	It("should handle multiple config files", func() {
		ign := NewEmptyIgnition(testIgnitionVersion)
		flavorSpec := &dpuprovisioningv1alpha1.DPUFlavorSpec{
			ConfigFiles: []dpuprovisioningv1alpha1.ConfigFile{
				{
					Path:        "/etc/mellanox/mlnx-bf.conf",
					Operation:   dpuprovisioningv1alpha1.FileOverride,
					Raw:         "ALLOW_SHARED_RQ=\"no\"",
					Permissions: "0644",
				},
				{
					Path:        "/etc/mellanox/mlnx-ovs.conf",
					Operation:   dpuprovisioningv1alpha1.FileOverride,
					Raw:         "CREATE_OVS_BRIDGES=\"no\"",
					Permissions: "0644",
				},
				{
					Path:        "/etc/mellanox/mlnx-sf.conf",
					Operation:   dpuprovisioningv1alpha1.FileOverride,
					Raw:         "",
					Permissions: "0644",
				},
			},
		}

		err := AddFlavorConfigFiles(ign, flavorSpec)
		Expect(err).NotTo(HaveOccurred())
		Expect(ign.Storage.Files).To(HaveLen(3))
		Expect(ign.Storage.Files[0].Path).To(Equal("/etc/mellanox/mlnx-bf.conf"))
		Expect(ign.Storage.Files[1].Path).To(Equal("/etc/mellanox/mlnx-ovs.conf"))
		Expect(ign.Storage.Files[2].Path).To(Equal("/etc/mellanox/mlnx-sf.conf"))
	})

	It("should handle empty raw content (for creating empty files)", func() {
		ign := NewEmptyIgnition(testIgnitionVersion)
		flavorSpec := &dpuprovisioningv1alpha1.DPUFlavorSpec{
			ConfigFiles: []dpuprovisioningv1alpha1.ConfigFile{
				{
					Path:        "/etc/mellanox/mlnx-sf.conf",
					Operation:   dpuprovisioningv1alpha1.FileOverride,
					Raw:         "",
					Permissions: "0644",
				},
			},
		}

		err := AddFlavorConfigFiles(ign, flavorSpec)
		Expect(err).NotTo(HaveOccurred())
		Expect(ign.Storage.Files).To(HaveLen(1))

		source := *ign.Storage.Files[0].Contents.Source
		b64 := strings.TrimPrefix(source, "data:text/plain;charset=utf-8;base64,")
		decoded, err := base64.StdEncoding.DecodeString(b64)
		Expect(err).NotTo(HaveOccurred())
		Expect(string(decoded)).To(BeEmpty())
	})

	It("should use default permissions when not specified", func() {
		ign := NewEmptyIgnition(testIgnitionVersion)
		flavorSpec := &dpuprovisioningv1alpha1.DPUFlavorSpec{
			ConfigFiles: []dpuprovisioningv1alpha1.ConfigFile{
				{
					Path:      "/etc/some/config",
					Operation: dpuprovisioningv1alpha1.FileOverride,
					Raw:       "content",
				},
			},
		}

		err := AddFlavorConfigFiles(ign, flavorSpec)
		Expect(err).NotTo(HaveOccurred())
		Expect(*ign.Storage.Files[0].Mode).To(Equal(0644))
	})

	It("should parse permissions correctly", func() {
		ign := NewEmptyIgnition(testIgnitionVersion)
		flavorSpec := &dpuprovisioningv1alpha1.DPUFlavorSpec{
			ConfigFiles: []dpuprovisioningv1alpha1.ConfigFile{
				{
					Path:        "/usr/local/bin/custom-script.sh",
					Operation:   dpuprovisioningv1alpha1.FileOverride,
					Raw:         "#!/bin/bash\necho hello",
					Permissions: "0755",
				},
			},
		}

		err := AddFlavorConfigFiles(ign, flavorSpec)
		Expect(err).NotTo(HaveOccurred())
		Expect(*ign.Storage.Files[0].Mode).To(Equal(0755))
	})

	It("should return error for invalid permissions", func() {
		ign := NewEmptyIgnition(testIgnitionVersion)
		flavorSpec := &dpuprovisioningv1alpha1.DPUFlavorSpec{
			ConfigFiles: []dpuprovisioningv1alpha1.ConfigFile{
				{
					Path:        "/etc/test",
					Operation:   dpuprovisioningv1alpha1.FileOverride,
					Raw:         "content",
					Permissions: "invalid",
				},
			},
		}

		err := AddFlavorConfigFiles(ign, flavorSpec)
		Expect(err).To(HaveOccurred())
		Expect(err.Error()).To(ContainSubstring("invalid permissions"))
	})

	It("should skip config files with empty path", func() {
		ign := NewEmptyIgnition(testIgnitionVersion)
		flavorSpec := &dpuprovisioningv1alpha1.DPUFlavorSpec{
			ConfigFiles: []dpuprovisioningv1alpha1.ConfigFile{
				{
					Path:        "",
					Operation:   dpuprovisioningv1alpha1.FileOverride,
					Raw:         "content",
					Permissions: "0644",
				},
			},
		}

		err := AddFlavorConfigFiles(ign, flavorSpec)
		Expect(err).NotTo(HaveOccurred())
		Expect(ign.Storage.Files).To(BeEmpty())
	})

	It("should be a no-op when ConfigFiles is empty", func() {
		ign := NewEmptyIgnition(testIgnitionVersion)
		flavorSpec := &dpuprovisioningv1alpha1.DPUFlavorSpec{}

		err := AddFlavorConfigFiles(ign, flavorSpec)
		Expect(err).NotTo(HaveOccurred())
		Expect(ign.Storage.Files).To(BeEmpty())
	})

	It("should handle mixed override and append operations", func() {
		ign := NewEmptyIgnition(testIgnitionVersion)
		flavorSpec := &dpuprovisioningv1alpha1.DPUFlavorSpec{
			ConfigFiles: []dpuprovisioningv1alpha1.ConfigFile{
				{
					Path:        "/etc/mellanox/mlnx-bf.conf",
					Operation:   dpuprovisioningv1alpha1.FileOverride,
					Raw:         "ALLOW_SHARED_RQ=\"no\"",
					Permissions: "0644",
				},
				{
					Path:        "/etc/sysctl.d/99-custom.conf",
					Operation:   dpuprovisioningv1alpha1.FileAppend,
					Raw:         "net.ipv4.ip_forward=1\n",
					Permissions: "0644",
				},
			},
		}

		err := AddFlavorConfigFiles(ign, flavorSpec)
		Expect(err).NotTo(HaveOccurred())
		Expect(ign.Storage.Files).To(HaveLen(2))

		Expect(ign.Storage.Files[0].Path).To(Equal("/etc/mellanox/mlnx-bf.conf"))
		Expect(*ign.Storage.Files[0].Overwrite).To(BeTrue())
		Expect(ign.Storage.Files[0].Append).To(BeEmpty())

		Expect(ign.Storage.Files[1].Path).To(Equal("/etc/sysctl.d/99-custom.conf"))
		Expect(ign.Storage.Files[1].Overwrite).To(BeNil())
		Expect(ign.Storage.Files[1].Append).To(HaveLen(1))
	})
})
