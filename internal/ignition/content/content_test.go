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

package content

import (
	"testing"

	igntypes "github.com/coreos/ignition/v2/config/v3_4/types"
	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/ignition"
)

func TestContent(t *testing.T) {
	RegisterFailHandler(Fail)
	RunSpecs(t, "Content Suite")
}

// mockProvider implements ContentProvider for testing
type mockProvider struct {
	files        []FileDefinition
	links        []LinkDefinition
	systemdUnits []SystemdUnitDefinition
	systemdErr   error
}

func (m *mockProvider) GetFiles() []FileDefinition {
	return m.files
}

func (m *mockProvider) GetLinks() []LinkDefinition {
	return m.links
}

func (m *mockProvider) GetSystemdUnits() ([]SystemdUnitDefinition, error) {
	return m.systemdUnits, m.systemdErr
}

var _ = Describe("AddFiles", func() {
	It("should add files with string content source (data URI)", func() {
		ign := ignition.NewEmptyIgnition("3.4.0")
		provider := &mockProvider{
			files: []FileDefinition{
				{Path: "/etc/test.conf", Mode: 0644, ContentSource: "data:,test-content"},
			},
		}

		Expect(AddFiles(ign, provider)).To(Succeed())

		Expect(ign.Storage.Files).To(HaveLen(1))
		file := ign.Storage.Files[0]
		Expect(file.Path).To(Equal("/etc/test.conf"))
		Expect(*file.Mode).To(Equal(0644))
		Expect(*file.Contents.Source).To(Equal("data:,test-content"))
		Expect(*file.Overwrite).To(BeTrue())
	})

	It("should add files with byte content source (gzip+base64 encoded)", func() {
		ign := ignition.NewEmptyIgnition("3.4.0")
		provider := &mockProvider{
			files: []FileDefinition{
				{Path: "/usr/local/bin/script.sh", Mode: 0755, ContentSource: []byte("#!/bin/bash\necho hello")},
			},
		}

		Expect(AddFiles(ign, provider)).To(Succeed())

		Expect(ign.Storage.Files).To(HaveLen(1))
		file := ign.Storage.Files[0]
		Expect(file.Path).To(Equal("/usr/local/bin/script.sh"))
		Expect(*file.Mode).To(Equal(0755))
		Expect(*file.Contents.Source).To(HavePrefix("data:;base64,"))
		Expect(file.Contents.Compression).NotTo(BeNil())
		Expect(*file.Contents.Compression).To(Equal("gzip"))
	})

	It("should return error for invalid content source type", func() {
		ign := ignition.NewEmptyIgnition("3.4.0")
		provider := &mockProvider{
			files: []FileDefinition{
				{Path: "/etc/bad", Mode: 0644, ContentSource: 12345},
			},
		}

		err := AddFiles(ign, provider)
		Expect(err).To(HaveOccurred())
		Expect(err.Error()).To(ContainSubstring("invalid content source type"))
	})

	It("should set correct file metadata", func() {
		ign := ignition.NewEmptyIgnition("3.4.0")
		provider := &mockProvider{
			files: []FileDefinition{
				{Path: "/etc/a.conf", Mode: 0600, ContentSource: "data:,a"},
				{Path: "/etc/b.conf", Mode: 0755, ContentSource: "data:,b"},
			},
		}

		Expect(AddFiles(ign, provider)).To(Succeed())

		Expect(ign.Storage.Files).To(HaveLen(2))
		Expect(ign.Storage.Files[0].Path).To(Equal("/etc/a.conf"))
		Expect(*ign.Storage.Files[0].Mode).To(Equal(0600))
		Expect(ign.Storage.Files[1].Path).To(Equal("/etc/b.conf"))
		Expect(*ign.Storage.Files[1].Mode).To(Equal(0755))
	})
})

var _ = Describe("AddSystemdUnits", func() {
	It("should add systemd units to ignition", func() {
		ign := ignition.NewEmptyIgnition("3.4.0")
		provider := &mockProvider{
			systemdUnits: []SystemdUnitDefinition{
				{Name: "test.service", Contents: []byte("[Unit]\nDescription=Test\n[Service]\nExecStart=/bin/true\n[Install]\nWantedBy=multi-user.target")},
			},
		}

		Expect(AddSystemdUnits(ign, provider)).To(Succeed())

		Expect(ign.Systemd.Units).To(HaveLen(1))
		unit := ign.Systemd.Units[0]
		Expect(unit.Name).To(Equal("test.service"))
		Expect(*unit.Contents).To(ContainSubstring("Description=Test"))
	})

	It("should set all units as enabled", func() {
		ign := ignition.NewEmptyIgnition("3.4.0")
		provider := &mockProvider{
			systemdUnits: []SystemdUnitDefinition{
				{Name: "a.service", Contents: []byte("[Unit]\nDescription=A")},
				{Name: "b.service", Contents: []byte("[Unit]\nDescription=B")},
			},
		}

		Expect(AddSystemdUnits(ign, provider)).To(Succeed())

		for _, unit := range ign.Systemd.Units {
			Expect(*unit.Enabled).To(BeTrue())
		}
	})

	It("should handle nil systemd FS (no units)", func() {
		ign := ignition.NewEmptyIgnition("3.4.0")
		provider := &EmbeddedProvider{
			Files:     nil,
			SystemdFS: nil,
		}

		Expect(AddSystemdUnits(ign, provider)).To(Succeed())
		Expect(ign.Systemd.Units).To(BeEmpty())
	})
})

var _ = Describe("AddContent", func() {
	It("should add both files and systemd units", func() {
		ign := ignition.NewEmptyIgnition("3.4.0")
		provider := &mockProvider{
			files: []FileDefinition{
				{Path: "/etc/test.conf", Mode: 0644, ContentSource: "data:,content"},
			},
			systemdUnits: []SystemdUnitDefinition{
				{Name: "test.service", Contents: []byte("[Unit]\nDescription=Test")},
			},
		}

		Expect(AddContent(ign, provider)).To(Succeed())

		Expect(ign.Storage.Files).To(HaveLen(1))
		Expect(ign.Systemd.Units).To(HaveLen(1))
	})
})

var _ = Describe("AddKernelArgs", func() {
	It("should add kernel argument template", func() {
		ign := ignition.NewEmptyIgnition("3.4.0")

		AddKernelArgs(ign)

		Expect(ign.KernelArguments.ShouldExist).To(HaveLen(1))
		Expect(string(ign.KernelArguments.ShouldExist[0])).To(Equal("{{.KernelParameters}}"))
	})
})

// Verify EmbeddedProvider satisfies ContentProvider
var _ ContentProvider = &EmbeddedProvider{}
var _ ContentProvider = &mockProvider{}

// Suppress unused import
var _ = igntypes.Config{}
