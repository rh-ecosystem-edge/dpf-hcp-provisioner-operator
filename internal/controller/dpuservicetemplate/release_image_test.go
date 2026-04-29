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

package dpuservicetemplate

import (
	"archive/tar"
	"bytes"
	"encoding/json"
	"io"

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
)

func buildTestTar(files map[string][]byte) io.Reader {
	var buf bytes.Buffer
	tw := tar.NewWriter(&buf)
	for name, content := range files {
		hdr := &tar.Header{
			Name: name,
			Mode: 0600,
			Size: int64(len(content)),
		}
		Expect(tw.WriteHeader(hdr)).To(Succeed())
		_, err := tw.Write(content)
		Expect(err).NotTo(HaveOccurred())
	}
	Expect(tw.Close()).To(Succeed())
	return &buf
}

var _ = Describe("Release Image Utilities", func() {
	Describe("findComponentInImageReferences", func() {
		It("should find a component by name", func() {
			refs := imageReferences{}
			refs.Spec.Tags = []struct {
				Name string `json:"name"`
				From struct {
					Name string `json:"name"`
				} `json:"from"`
			}{
				{Name: "ovn-kubernetes", From: struct {
					Name string `json:"name"`
				}{Name: "quay.io/openshift-release-dev/ocp-v4.0-art-dev@sha256:abc123"}},
				{Name: "kube-proxy", From: struct {
					Name string `json:"name"`
				}{Name: "quay.io/other:v1"}},
			}

			data, err := json.Marshal(refs)
			Expect(err).NotTo(HaveOccurred())

			reader := buildTestTar(map[string][]byte{
				imageReferencesPath: data,
			})

			result, err := findComponentInImageReferences(reader, "ovn-kubernetes")
			Expect(err).NotTo(HaveOccurred())
			Expect(result).To(Equal("quay.io/openshift-release-dev/ocp-v4.0-art-dev@sha256:abc123"))
		})

		It("should return error when component is not found", func() {
			refs := imageReferences{}
			refs.Spec.Tags = []struct {
				Name string `json:"name"`
				From struct {
					Name string `json:"name"`
				} `json:"from"`
			}{
				{Name: "kube-proxy", From: struct {
					Name string `json:"name"`
				}{Name: "quay.io/other:v1"}},
			}

			data, err := json.Marshal(refs)
			Expect(err).NotTo(HaveOccurred())

			reader := buildTestTar(map[string][]byte{
				imageReferencesPath: data,
			})

			_, err = findComponentInImageReferences(reader, "ovn-kubernetes")
			Expect(err).To(HaveOccurred())
			Expect(err.Error()).To(ContainSubstring("not found"))
		})

		It("should return error when image-references file is missing", func() {
			reader := buildTestTar(map[string][]byte{
				"some-other-file": []byte("data"),
			})

			_, err := findComponentInImageReferences(reader, "ovn-kubernetes")
			Expect(err).To(HaveOccurred())
			Expect(err.Error()).To(ContainSubstring("not found in release image"))
		})

		It("should return error when component has empty image reference", func() {
			refs := imageReferences{}
			refs.Spec.Tags = []struct {
				Name string `json:"name"`
				From struct {
					Name string `json:"name"`
				} `json:"from"`
			}{
				{Name: "ovn-kubernetes", From: struct {
					Name string `json:"name"`
				}{Name: ""}},
			}

			data, err := json.Marshal(refs)
			Expect(err).NotTo(HaveOccurred())

			reader := buildTestTar(map[string][]byte{
				imageReferencesPath: data,
			})

			_, err = findComponentInImageReferences(reader, "ovn-kubernetes")
			Expect(err).To(HaveOccurred())
			Expect(err.Error()).To(ContainSubstring("empty image reference"))
		})
	})

	Describe("splitImage", func() {
		It("should split image with tag", func() {
			repo, tag, err := splitImage("quay.io/openshift/ovn:v4.19.0")
			Expect(err).NotTo(HaveOccurred())
			Expect(repo).To(Equal("quay.io/openshift/ovn"))
			Expect(tag).To(Equal("v4.19.0"))
		})

		It("should split image with sha256 digest", func() {
			repo, tag, err := splitImage("quay.io/openshift-release-dev/ocp-v4.0-art-dev@sha256:abcdef1234567890")
			Expect(err).NotTo(HaveOccurred())
			Expect(repo).To(Equal("quay.io/openshift-release-dev/ocp-v4.0-art-dev@sha256"))
			Expect(tag).To(Equal("abcdef1234567890"))
		})

		It("should default to 'latest' tag when no tag present", func() {
			repo, tag, err := splitImage("quay.io/openshift/ovn")
			Expect(err).NotTo(HaveOccurred())
			Expect(repo).To(Equal("quay.io/openshift/ovn"))
			Expect(tag).To(Equal("latest"))
		})

		It("should return error for empty image", func() {
			_, _, err := splitImage("")
			Expect(err).To(HaveOccurred())
			Expect(err.Error()).To(ContainSubstring("empty image reference"))
		})
	})
})
