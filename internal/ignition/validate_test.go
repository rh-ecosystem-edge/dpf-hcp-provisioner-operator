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
	igntypes "github.com/coreos/ignition/v2/config/v3_4/types"
	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
)

var _ = Describe("ValidateConfig", func() {
	It("should accept a valid empty ignition config", func() {
		cfg := NewEmptyIgnition(testIgnitionVersion)
		Expect(ValidateConfig(cfg)).To(Succeed())
	})

	It("should accept a valid ignition config with files", func() {
		cfg := NewEmptyIgnition(testIgnitionVersion)
		source := "data:,hello"
		mode := 0644
		overwrite := true
		cfg.Storage.Files = append(cfg.Storage.Files, igntypes.File{
			Node: igntypes.Node{Path: "/etc/test-file", Overwrite: &overwrite},
			FileEmbedded1: igntypes.FileEmbedded1{
				Contents: igntypes.Resource{Source: &source},
				Mode:     &mode,
			},
		})
		Expect(ValidateConfig(cfg)).To(Succeed())
	})

	It("should reject ignition config with duplicate file paths", func() {
		cfg := NewEmptyIgnition(testIgnitionVersion)
		source := "data:,content1"
		mode := 0644
		overwrite := true
		file := igntypes.File{
			Node: igntypes.Node{Path: "/etc/duplicate-file", Overwrite: &overwrite},
			FileEmbedded1: igntypes.FileEmbedded1{
				Contents: igntypes.Resource{Source: &source},
				Mode:     &mode,
			},
		}
		cfg.Storage.Files = append(cfg.Storage.Files, file, file)

		err := ValidateConfig(cfg)
		Expect(err).To(HaveOccurred())
		Expect(err.Error()).To(ContainSubstring("duplicate"))
	})

	It("should reject ignition config with duplicate systemd unit names", func() {
		cfg := NewEmptyIgnition(testIgnitionVersion)
		enabled := true
		contents := "[Unit]\nDescription=Test\n[Service]\nExecStart=/bin/true\n[Install]\nWantedBy=multi-user.target"
		unit := igntypes.Unit{
			Name:     "duplicate.service",
			Enabled:  &enabled,
			Contents: &contents,
		}
		cfg.Systemd.Units = append(cfg.Systemd.Units, unit, unit)

		err := ValidateConfig(cfg)
		Expect(err).To(HaveOccurred())
		Expect(err.Error()).To(ContainSubstring("duplicate"))
	})

	It("should accept ignition config with multiple unique files", func() {
		cfg := NewEmptyIgnition(testIgnitionVersion)
		mode := 0644
		overwrite := true
		for i := 0; i < 10; i++ {
			source := "data:,content"
			cfg.Storage.Files = append(cfg.Storage.Files, igntypes.File{
				Node: igntypes.Node{Path: "/etc/file-" + string(rune('a'+i)), Overwrite: &overwrite},
				FileEmbedded1: igntypes.FileEmbedded1{
					Contents: igntypes.Resource{Source: &source},
					Mode:     &mode,
				},
			})
		}
		Expect(ValidateConfig(cfg)).To(Succeed())
	})

	It("should reject ignition config with invalid version", func() {
		cfg := &igntypes.Config{
			Ignition: igntypes.Ignition{
				Version: "2.0.0",
			},
		}
		err := ValidateConfig(cfg)
		Expect(err).To(HaveOccurred())
	})
})
