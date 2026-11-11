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

// Command registry generates golden fixtures for the Rust port of the
// go-containerregistry subset (crates/registry).
//
// Usage: go run -mod=vendor ./hack/golden/registry [outdir]
package main

import (
	"archive/tar"
	"bytes"
	"compress/gzip"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"time"

	authchallenge "github.com/docker/distribution/registry/client/auth/challenge"
	"github.com/google/go-containerregistry/pkg/authn"
	"github.com/google/go-containerregistry/pkg/name"
	v1 "github.com/google/go-containerregistry/pkg/v1"
	"github.com/google/go-containerregistry/pkg/v1/mutate"
	"github.com/google/go-containerregistry/pkg/v1/partial"
	"github.com/google/go-containerregistry/pkg/v1/remote/transport"
	"github.com/google/go-containerregistry/pkg/v1/types"
	"github.com/klauspost/compress/zstd"
)

func must(err error) {
	if err != nil {
		panic(err)
	}
}

func writeJSON(dir, file string, v any) {
	b, err := json.MarshalIndent(v, "", "  ")
	must(err)
	must(os.WriteFile(filepath.Join(dir, file), append(b, '\n'), 0o644))
}

// ---------------------------------------------------------------- name

var nameInputs = []string{
	"",
	"ubuntu",
	"ubuntu:latest",
	"ubuntu:22.04",
	"library/ubuntu",
	"docker.io/ubuntu",
	"docker.io/library/ubuntu:20.04",
	"index.docker.io/ubuntu",
	"index.docker.io/library/ubuntu",
	"index.docker.io/foo/bar:baz",
	"quay.io/openshift-release-dev/ocp-release:4.19.0-x86_64",
	"quay.io/openshift-release-dev/ocp-release:4.19.0-multi",
	"quay.io/openshift-release-dev/ocp-release@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
	"quay.io/openshift-release-dev/ocp-release:4.19.0@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
	"quay.io/openshift-release-dev/ocp-release@sha256:0123456789ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef",
	"quay.io/openshift-release-dev/ocp-release@sha256:0123",
	"quay.io/openshift-release-dev/ocp-release@sha512:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
	"quay.io/foo@bar@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
	"quay.io/edge-infrastructure/bluefield-ocp:4.19.0",
	"quay.io/edge-infrastructure/bluefield-ocp",
	"quay.io",
	"quay.io:443",
	"quay.io/",
	"quay.io/a",
	"quay.io/ab",
	"quay.io/AB",
	"quay.io/ab:",
	"quay.io/ab:tag:",
	"quay.io/ab::tag",
	"registry.example.com:5000/foo/bar:v1",
	"registry.example.com:5000/foo/bar",
	"registry.example.com:5000",
	"registry.example.com:5000:tag",
	"registry.example.com:port/foo",
	"registry.example.com:/foo",
	"localhost/foo",
	"localhost:5000/foo",
	"localhost:5000/foo:bar",
	"localhost",
	"127.0.0.1:5000/foo",
	"127.0.0.1/foo",
	"127.0.0.10:5000/foo",
	"10.1.2.3:5000/foo",
	"10.1.2.3/foo",
	"172.16.0.1/foo",
	"172.32.0.1/foo",
	"192.168.1.1:80/foo",
	"192.169.1.1/foo",
	"8.8.8.8/foo",
	"010.1.2.3/foo",
	"[::1]:5000/foo",
	"[::1]/foo",
	"[fd00::1]:5000/foo",
	"[1.2.3.4]:5000/foo",
	"[::ffff:1.2.3.4]/foo",
	"[::1:5000/foo",
	"a[::1]/foo",
	"foo.local/bar",
	"foo.local:5000/bar",
	"foo.localhost/bar",
	"foo.localhost:123456/bar",
	"foo.local:/bar",
	"my.registry.local:65535/bar:baz",
	"example.com/a/b/c/d/e:tag",
	"example.com/a//b",
	"example.com/a/b/",
	"example.com/-a",
	"example.com/a_b-c.d",
	"example.com/UPPER",
	"example.com/foo:UPPER_tag-1.0",
	"example.com/foo:tag!",
	"example.com/foo:" + strings.Repeat("t", 128),
	"example.com/foo:" + strings.Repeat("t", 129),
	"example.com/" + strings.Repeat("r", 255),
	"example.com/" + strings.Repeat("r", 256),
	"example.com/ünïcode",
	"exämple.com/foo",
	"ex ample.com/foo",
	"ex_ample.com/foo",
	"ex%41mple.com/foo",
	"ex%2fmple.com/foo",
	"user@example.com/foo",
	"example.com?x/foo",
	"example.com#x/foo",
	"exa}mple.com/foo",
	"exa]mple.com/foo",
	"exa!mple.com/foo",
	"a:b:c/foo",
	"a:1:2/foo",
	"example.com:99999999/foo",
	"foo/bar/baz",
	"foo:bar/baz",
	"foo.bar/baz",
	"foo:5000",
	"foo:5000/bar",
	"foo@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
	"foo:bar@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
	"@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
	"docker.io/foo@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
	"gcr.io/project/image@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
	"gcr.io/project/image:tag@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
	"gcr.io/project/image:t@g@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
	"gcr.io/project/image@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdeg",
	"gcr.io/project/image@sha256:",
	"gcr.io/project/image@",
	"gcr.io/project/image@sha256",
	":tag",
	"/foo",
	"foo/",
	"a",
	"ab",
	"a/b",
	"registry.redhat.io/ubi9/ubi-minimal:latest",
	"registry.ci.openshift.org/ocp/release:4.19.0-0.nightly-2025-01-01-000000",
	"quay.io/openshift-release-dev/ocp-release:4.19.0-aarch64",
	"Quay.IO/foo/bar",
	"quay.io/foo/bar:4.19.0\n",
	"quay.io/foo/bar:4.19.0 ",
	"quay.io/foo\tbar",
	"docker.io",
	"docker.io/foo",
	"docker.io:443/foo",
	"index.docker.io:443/foo",
}

type nameResult struct {
	OK            bool   `json:"ok"`
	Err           string `json:"err,omitempty"`
	IsBadName     bool   `json:"isBadName,omitempty"`
	Kind          string `json:"kind,omitempty"`
	String        string `json:"string,omitempty"`
	Name          string `json:"name,omitempty"`
	RegistryStr   string `json:"registryStr,omitempty"`
	RepositoryStr string `json:"repositoryStr,omitempty"`
	Identifier    string `json:"identifier,omitempty"`
	Context       string `json:"context,omitempty"`
	Scheme        string `json:"scheme,omitempty"`
	Scope         string `json:"scope,omitempty"`
}

type nameCase struct {
	Func   string     `json:"func"`
	Opts   string     `json:"opts"`
	Input  string     `json:"input"`
	Result nameResult `json:"result"`
}

func optSets() map[string][]name.Option {
	return map[string][]name.Option{
		"default":         nil,
		"strict":          {name.StrictValidation},
		"insecure":        {name.Insecure},
		"defaultRegistry": {name.WithDefaultRegistry("registry.example.com")},
		"defaultTag":      {name.WithDefaultTag("v1")},
		"weak":            {name.WeakValidation},
	}
}

func errResult(err error) nameResult {
	return nameResult{OK: false, Err: err.Error(), IsBadName: name.IsErrBadName(err)}
}

func refResult(r name.Reference) nameResult {
	res := nameResult{
		OK:            true,
		String:        r.String(),
		Name:          r.Name(),
		RegistryStr:   r.Context().RegistryStr(),
		RepositoryStr: r.Context().RepositoryStr(),
		Identifier:    r.Identifier(),
		Context:       r.Context().String(),
		Scheme:        r.Context().Scheme(),
		Scope:         r.Scope(transport.PullScope),
	}
	switch r.(type) {
	case name.Tag:
		res.Kind = "tag"
	case name.Digest:
		res.Kind = "digest"
	}
	return res
}

func genName(dir string) {
	var cases []nameCase
	optNames := []string{"default", "strict", "insecure", "defaultRegistry", "defaultTag"}
	sets := optSets()
	for _, on := range optNames {
		opts := sets[on]
		for _, in := range nameInputs {
			// ParseReference
			if r, err := name.ParseReference(in, opts...); err != nil {
				cases = append(cases, nameCase{"ParseReference", on, in, errResult(err)})
			} else {
				cases = append(cases, nameCase{"ParseReference", on, in, refResult(r)})
			}
			// NewTag
			if r, err := name.NewTag(in, opts...); err != nil {
				cases = append(cases, nameCase{"NewTag", on, in, errResult(err)})
			} else {
				cases = append(cases, nameCase{"NewTag", on, in, refResult(r)})
			}
			// NewDigest
			if r, err := name.NewDigest(in, opts...); err != nil {
				cases = append(cases, nameCase{"NewDigest", on, in, errResult(err)})
			} else {
				cases = append(cases, nameCase{"NewDigest", on, in, refResult(r)})
			}
			// NewRepository
			if r, err := name.NewRepository(in, opts...); err != nil {
				cases = append(cases, nameCase{"NewRepository", on, in, errResult(err)})
			} else {
				cases = append(cases, nameCase{"NewRepository", on, in, nameResult{
					OK:            true,
					String:        r.String(),
					Name:          r.Name(),
					RegistryStr:   r.RegistryStr(),
					RepositoryStr: r.RepositoryStr(),
					Scheme:        r.Scheme(),
					Scope:         r.Scope(transport.PullScope),
				}})
			}
			// NewRegistry
			if r, err := name.NewRegistry(in, opts...); err != nil {
				cases = append(cases, nameCase{"NewRegistry", on, in, errResult(err)})
			} else {
				cases = append(cases, nameCase{"NewRegistry", on, in, nameResult{
					OK:          true,
					String:      r.String(),
					Name:        r.Name(),
					RegistryStr: r.RegistryStr(),
					Scheme:      r.Scheme(),
					Scope:       r.Scope(transport.PullScope),
				}})
			}
		}
	}
	// Repository.Tag / Repository.Digest / Registry.Repo helpers.
	repo, err := name.NewRepository("quay.io/foo/bar")
	must(err)
	cases = append(cases, nameCase{"Repository.Tag", "default", "quay.io/foo/bar|v1", refResult(repo.Tag("v1"))})
	cases = append(cases, nameCase{"Repository.Digest", "default", "quay.io/foo/bar|sha256:abc", refResult(repo.Digest("sha256:abc"))})
	reg, err := name.NewRegistry("docker.io")
	must(err)
	r2 := reg.Repo("a", "b/", "/c")
	cases = append(cases, nameCase{"Registry.Repo", "default", "docker.io|a,b/,/c", nameResult{OK: true, String: r2.String(), RepositoryStr: r2.RepositoryStr()}})
	r3 := reg.Repo("ubuntu")
	cases = append(cases, nameCase{"Registry.Repo", "default", "docker.io|ubuntu", nameResult{OK: true, String: r3.String(), RepositoryStr: r3.RepositoryStr()}})
	writeJSON(dir, "name_golden.json", cases)
}

// ---------------------------------------------------------------- authn

type authCase struct {
	Input  string           `json:"input"`
	OK     bool             `json:"ok"`
	Err    string           `json:"err,omitempty"`
	Config authn.AuthConfig `json:"config"`
	// Fields as decoded (MarshalJSON would recompute auth, so record raw).
	Username      string `json:"username"`
	Password      string `json:"password"`
	Auth          string `json:"auth"`
	IdentityToken string `json:"identitytoken"`
	RegistryToken string `json:"registrytoken"`
	Marshaled     string `json:"marshaled"`
}

var authInputs = []string{
	`{}`,
	`{"username":"u","password":"p"}`,
	`{"username":"u"}`,
	`{"password":"p"}`,
	`{"auth":"dTpw"}`,
	`{"auth":"dTpw","username":"x","password":"y"}`,
	`{"auth":"dXNlcjpwYXNzd29yZA=="}`,
	`{"auth":"dXNlcjpwYXNzd29yZA"}`,
	`{"auth":"dXNlcjpwYXNzd29yZA="}`,
	`{"auth":"dXNlcjpwYXNz\nd29yZA=="}`,
	`{"auth":"dXNlcjpwYXNz\r\nd29yZA=="}`,
	`{"auth":"  dXNlcjpwYXNzd29yZA== "}`,
	`{"auth":"dXNlcg=="}`,
	`{"auth":"dXNlcg"}`,
	`{"auth":"!!!!"}`,
	`{"auth":"dXNlcjpwYXNz!"}`,
	`{"auth":"dXNlcjp="}`,
	`{"auth":"dXNlcjp=x"}`,
	`{"auth":"dXNlcjp=="}`,
	`{"auth":"dXNlcjpw==="}`,
	`{"auth":"="}`,
	`{"auth":"a"}`,
	`{"auth":"ab"}`,
	`{"auth":"abc"}`,
	`{"auth":"abcd"}`,
	`{"auth":"OjpwYXNzOndvcmQ="}`,
	`{"auth":"OjpwYXNzOnd vcmQ="}`,
	`{"auth":"dTpw","identitytoken":"idt","registrytoken":"rt"}`,
	`{"identitytoken":"idt"}`,
	`{"registrytoken":"rt"}`,
	`{"Username":"U","PASSWORD":"P"}`,
	`{"username":null,"password":"p"}`,
	`{"username":"u","password":"p","email":"e@x"}`,
	`{"username":"üser","password":"päss"}`,
	`{"auth":"w7xzZXI6cMOkc3M="}`,
	`{"username":5,"password":"p"}`,
	`{"auth":5}`,
	`{"auth":"dTpw","username":true}`,
	`null`,
	`"str"`,
	`[]`,
	`{"username":"<a&b>","password":"\u2028"}`,
}

func genAuthn(dir string) {
	var cases []authCase
	for _, in := range authInputs {
		var cfg authn.AuthConfig
		err := json.Unmarshal([]byte(in), &cfg)
		c := authCase{Input: in, OK: err == nil, Username: cfg.Username, Password: cfg.Password, Auth: cfg.Auth, IdentityToken: cfg.IdentityToken, RegistryToken: cfg.RegistryToken}
		if err != nil {
			c.Err = err.Error()
		}
		b, merr := json.Marshal(cfg)
		must(merr)
		c.Marshaled = string(b)
		cases = append(cases, c)
	}
	writeJSON(dir, "authn_golden.json", cases)
}

// ---------------------------------------------------------------- challenge

type challengeCase struct {
	Headers    []string                  `json:"headers"`
	Challenges []authchallenge.Challenge `json:"challenges"`
}

var challengeInputs = [][]string{
	{`Bearer realm="https://auth.example.com/token",service="registry.example.com"`},
	{`Bearer realm="https://auth.example.com/token",service="registry.example.com",scope="repository:foo/bar:pull"`},
	{`bearer Realm="https://auth.example.com/token", Service="x"`},
	{`Basic realm="Registry Realm"`},
	{`Basic`},
	{`Negotiate`, `Basic realm="x"`},
	{`Bearer realm=https://unquoted/token,service=svc`},
	{`Bearer realm="a\"b\\c",service="s"`},
	{`Bearer realm="unterminated`},
	{`Bearer realm=`},
	{`Bearer realm="x",,service="y"`},
	{`Bearer realm="x" service="y"`},
	{`Bearer  realm="x" ,  service="y"`},
	{`Bearer realm="x",service="y",error="insufficient_scope"`},
	{``},
	{`=foo`},
	{`Bearer realm="x",realm="y"`},
	{`Bearer realm="esc\"aped",service="s\\"`},
	{`Bearer realm="a\`},
	{`Bearer,realm="x"`},
	{`Bearer realm`},
}

func genChallenge(dir string) {
	var cases []challengeCase
	for _, hs := range challengeInputs {
		resp := &http.Response{StatusCode: http.StatusUnauthorized, Header: http.Header{}}
		for _, h := range hs {
			resp.Header.Add("WWW-Authenticate", h)
		}
		cases = append(cases, challengeCase{Headers: hs, Challenges: authchallenge.ResponseChallenges(resp)})
	}
	writeJSON(dir, "challenge_golden.json", cases)
}

// ---------------------------------------------------------------- transport.Error

type errorCase struct {
	Method    string `json:"method"`
	URL       string `json:"url"`
	Status    int    `json:"status"`
	Body      string `json:"body"`
	Error     string `json:"error"`
	Temporary bool   `json:"temporary"`
}

var errorInputs = []struct {
	method, url string
	status      int
	body        string
}{
	{"GET", "https://quay.io/v2/foo/bar/manifests/latest", 404, `{"errors":[{"code":"MANIFEST_UNKNOWN","message":"manifest unknown","detail":{"Tag":"latest"}}]}`},
	{"HEAD", "https://quay.io/v2/foo/bar/manifests/latest", 404, ``},
	{"GET", "https://quay.io/v2/foo/bar/manifests/latest", 404, ``},
	{"GET", "https://quay.io/v2/foo/bar/manifests/latest", 401, `{"errors":[{"code":"UNAUTHORIZED","message":"access to the requested resource is not authorized","detail":[{"Type":"repository","Name":"foo/bar","Action":"pull"}]}]}`},
	{"GET", "https://quay.io/v2/foo/bar/manifests/latest", 403, `{"errors":[{"code":"DENIED","message":"requested access to the resource is denied"}]}`},
	{"GET", "https://quay.io/v2/foo/bar/manifests/latest", 400, `{"errors":[{"code":"NAME_INVALID","message":"a"},{"code":"TAG_INVALID","message":"b","detail":"c"}]}`},
	{"GET", "https://quay.io/v2/foo/bar/manifests/latest", 500, `<html>oops</html>`},
	{"GET", "https://quay.io/v2/foo/bar/manifests/latest", 503, `{"errors":[{"code":"UNAVAILABLE","message":"x"}]}`},
	{"GET", "https://quay.io/v2/foo/bar/manifests/latest", 429, `{"errors":[{"code":"TOOMANYREQUESTS","message":"slow down"}]}`},
	{"GET", "https://quay.io/v2/foo/bar/manifests/latest", 429, `{"errors":[{"code":"TOOMANYREQUESTS","message":"slow down"},{"code":"DENIED","message":"no"}]}`},
	{"GET", "https://quay.io/v2/foo/bar/manifests/latest", 418, `{"errors":[]}`},
	{"GET", "https://quay.io/v2/foo/bar/manifests/latest", 599, ``},
	{"GET", "https://quay.io/v2/foo/bar/manifests/latest", 404, `{"errors":[{"code":"NAME_UNKNOWN","message":"repository name not known to registry","detail":{"name":"foo/bar"}}]}`},
	{"GET", "https://quay.io/v2/foo/bar/manifests/latest", 404, `{"errors":[{"code":"MANIFEST_UNKNOWN","detail":null}]}`},
	{"GET", "https://quay.io/v2/foo/bar/manifests/latest", 404, `{"errors":[{"code":"MANIFEST_UNKNOWN","message":"m","detail":1.5}]}`},
	{"GET", "https://quay.io/v2/foo/bar/manifests/latest", 404, `{"errors":[{"code":"MANIFEST_UNKNOWN","message":"m","detail":true}]}`},
	{"GET", "https://quay.io/v2/foo/bar/manifests/latest", 404, `{"errors":[{"code":"MANIFEST_UNKNOWN","message":"m","detail":{"b":[1,"x",null],"a":{"z":1}}}]}`},
	{"GET", "https://quay.io/v2/foo/bar/manifests/latest", 404, `{"errors":"notalist"}`},
	{"GET", "https://quay.io/v2/foo/bar/manifests/latest", 404, `{"errors":[{"code":5}]}`},
	{"GET", "https://quay.io/v2/auth?scope=repository%3Afoo%2Fbar%3Apull&service=quay.io&account=me&secret=x", 401, `{"details":"bad"}`},
	{"GET", "https://user:pass@quay.io/v2/auth?b=2&a=1&a=3&scope=s", 401, ``},
	{"GET", "https://quay.io/v2/auth?x", 401, ``},
	{"GET", "https://quay.io/v2/auth?=v&k=", 401, ``},
	{"GET", "https://quay.io/v2/auth?scope=a+b&scope=c%20d", 401, ``},
	{"POST", "https://auth.example.com/oauth2/token", 404, `not found`},
	{"GET", "http://127.0.0.1:5000/v2/", 502, `bad gateway`},
	{"GET", "https://quay.io/v2/foo/bar/blobs/sha256:abc", 408, ``},
	{"GET", "https://quay.io/v2/foo/bar/blobs/sha256:abc", 504, ``},
	{"GET", "https://quay.io/v2/foo/bar/blobs/sha256:abc", 499, ``},
	{"GET", "https://quay.io/v2/foo/bar/blobs/sha256:abc", 522, ``},
	{"GET", "https://quay.io/v2/foo/bar/blobs/sha256:abc", 200, ``},
	{"GET", "https://quay.io/v2/foo/bar/blobs/sha256:abc", 404, `{"errors":[{"code":"BLOB_UNKNOWN","message":"blob unknown to registry","detail":"sha256:abc"}]}`},
	{"GET", "https://quay.io/a%20b/c?q=%zz", 404, ``},
	{"GET", "https://quay.io/v2/x/manifests/y?n=1&last=a&digest=d&mount=m&from=f&other=o", 404, ``},
}

func genErrors(dir string) {
	var cases []errorCase
	for _, in := range errorInputs {
		u, err := url.Parse(in.url)
		must(err)
		req := &http.Request{Method: in.method, URL: u}
		resp := &http.Response{StatusCode: in.status, Request: req, Body: io.NopCloser(strings.NewReader(in.body)), Header: http.Header{}}
		e := transport.CheckError(resp, http.StatusOK)
		c := errorCase{Method: in.method, URL: in.url, Status: in.status, Body: in.body}
		if e != nil {
			c.Error = e.Error()
			if te, ok := e.(*transport.Error); ok {
				c.Temporary = te.Temporary()
			}
		}
		cases = append(cases, c)
	}
	writeJSON(dir, "transport_error_golden.json", cases)
}

// ---------------------------------------------------------------- mutate.Extract

type staticLayer struct {
	data []byte
	mt   types.MediaType
}

func (l *staticLayer) Digest() (v1.Hash, error) {
	h, _, err := v1.SHA256(bytes.NewReader(l.data))
	return h, err
}
func (l *staticLayer) Compressed() (io.ReadCloser, error) {
	return io.NopCloser(bytes.NewReader(l.data)), nil
}
func (l *staticLayer) Size() (int64, error)                { return int64(len(l.data)), nil }
func (l *staticLayer) MediaType() (types.MediaType, error) { return l.mt, nil }

// layersImage implements just enough of v1.Image for mutate.Extract.
type layersImage struct {
	v1.Image
	layers []v1.Layer
}

func (i *layersImage) Layers() ([]v1.Layer, error) { return i.layers, nil }

type tarEntry struct {
	Name     string `json:"name"`
	Typeflag byte   `json:"typeflag"`
	Linkname string `json:"linkname,omitempty"`
	Mode     int64  `json:"mode"`
	Size     int64  `json:"size"`
	Content  string `json:"content,omitempty"`
	// PAX records for long names etc. are generated by archive/tar automatically.
}

func buildTar(entries []tarEntry, opts ...func(*tar.Header)) []byte {
	var buf bytes.Buffer
	tw := tar.NewWriter(&buf)
	for _, e := range entries {
		hdr := &tar.Header{
			Name:     e.Name,
			Typeflag: e.Typeflag,
			Linkname: e.Linkname,
			Mode:     e.Mode,
			Size:     int64(len(e.Content)),
			Uid:      1000,
			Gid:      1000,
			Uname:    "user",
			Gname:    "group",
		}
		if e.Typeflag == tar.TypeDir || e.Typeflag == tar.TypeSymlink || e.Typeflag == tar.TypeLink {
			hdr.Size = 0
		}
		for _, o := range opts {
			o(hdr)
		}
		must(tw.WriteHeader(hdr))
		if hdr.Size > 0 {
			_, err := tw.Write([]byte(e.Content))
			must(err)
		}
	}
	must(tw.Close())
	return buf.Bytes()
}

// patchTypeflag rewrites the typeflag of the header block at off and fixes the checksum.
func patchTypeflag(t []byte, off int, flag byte) {
	blk := t[off : off+512]
	blk[156] = flag
	var sum int64
	for i, c := range blk {
		if 148 <= i && i < 156 {
			c = ' '
		}
		sum += int64(c)
	}
	copy(blk[148:156], []byte(fmt.Sprintf("%06o\x00 ", sum)))
}

func gz(b []byte) []byte {
	var buf bytes.Buffer
	zw := gzip.NewWriter(&buf)
	_, err := zw.Write(b)
	must(err)
	must(zw.Close())
	return buf.Bytes()
}

func zst(b []byte) []byte {
	var buf bytes.Buffer
	zw, err := zstd.NewWriter(&buf)
	must(err)
	_, err = zw.Write(b)
	must(err)
	must(zw.Close())
	return buf.Bytes()
}

type extractEntry struct {
	Name     string `json:"name"`
	Typeflag byte   `json:"typeflag"`
	Linkname string `json:"linkname,omitempty"`
	Mode     int64  `json:"mode"`
	Size     int64  `json:"size"`
	Uid      int    `json:"uid"`
	Gid      int    `json:"gid"`
	Uname    string `json:"uname,omitempty"`
	Gname    string `json:"gname,omitempty"`
	Mtime    int64  `json:"mtime"`
	Sha256   string `json:"sha256,omitempty"`
}

type extractCase struct {
	Name    string         `json:"name"`
	Layers  []string       `json:"layers"` // fixture file names, bottom-most first
	Entries []extractEntry `json:"entries"`
	Err     string         `json:"err,omitempty"`
}

func runExtract(layers [][]byte) ([]extractEntry, string) {
	var ls []v1.Layer
	for _, b := range layers {
		l, err := partial.CompressedToLayer(&staticLayer{data: b, mt: types.OCILayer})
		must(err)
		ls = append(ls, l)
	}
	rc := mutate.Extract(&layersImage{layers: ls})
	defer rc.Close()
	tr := tar.NewReader(rc)
	var out []extractEntry
	for {
		hdr, err := tr.Next()
		if err == io.EOF {
			return out, ""
		}
		if err != nil {
			return out, "next: " + err.Error()
		}
		data, err := io.ReadAll(tr)
		if err != nil {
			return out, "read: " + err.Error()
		}
		e := extractEntry{Name: hdr.Name, Typeflag: hdr.Typeflag, Linkname: hdr.Linkname, Mode: hdr.Mode, Size: hdr.Size,
			Uid: hdr.Uid, Gid: hdr.Gid, Uname: hdr.Uname, Gname: hdr.Gname, Mtime: hdr.ModTime.Unix()}
		if len(data) > 0 {
			s := sha256.Sum256(data)
			e.Sha256 = hex.EncodeToString(s[:])
		}
		out = append(out, e)
	}
}

func genExtract(dir string) {
	fixDir := filepath.Join(dir, "extract")
	must(os.MkdirAll(fixDir, 0o755))
	var cases []extractCase

	add := func(caseName string, layers [][]byte) {
		var files []string
		for i, l := range layers {
			f := fmt.Sprintf("%s-%d.bin", caseName, i)
			must(os.WriteFile(filepath.Join(fixDir, f), l, 0o644))
			files = append(files, f)
		}
		entries, err := runExtract(layers)
		cases = append(cases, extractCase{Name: caseName, Layers: files, Entries: entries, Err: err})
	}

	base := []tarEntry{
		{Name: "./", Typeflag: tar.TypeDir, Mode: 0o755},
		{Name: "./etc/", Typeflag: tar.TypeDir, Mode: 0o755},
		{Name: "./etc/os-release", Typeflag: tar.TypeReg, Mode: 0o644, Content: "ID=rhel\n"},
		{Name: "./etc/passwd", Typeflag: tar.TypeReg, Mode: 0o644, Content: "root:x:0:0\n"},
		{Name: "usr/", Typeflag: tar.TypeDir, Mode: 0o755},
		{Name: "usr/bin/", Typeflag: tar.TypeDir, Mode: 0o755},
		{Name: "usr/bin/tool", Typeflag: tar.TypeReg, Mode: 0o755, Content: "#!/bin/sh\necho base\n"},
		{Name: "usr/bin/link", Typeflag: tar.TypeSymlink, Linkname: "tool", Mode: 0o777},
		{Name: "usr/bin/hard", Typeflag: tar.TypeLink, Linkname: "usr/bin/tool", Mode: 0o755},
		{Name: "opt/", Typeflag: tar.TypeDir, Mode: 0o755},
		{Name: "opt/app/", Typeflag: tar.TypeDir, Mode: 0o755},
		{Name: "opt/app/a.txt", Typeflag: tar.TypeReg, Mode: 0o644, Content: "a"},
		{Name: "opt/app/b.txt", Typeflag: tar.TypeReg, Mode: 0o644, Content: "b"},
		{Name: "opt/app/sub/", Typeflag: tar.TypeDir, Mode: 0o755},
		{Name: "opt/app/sub/c.txt", Typeflag: tar.TypeReg, Mode: 0o644, Content: "c"},
		{Name: "var/empty", Typeflag: tar.TypeReg, Mode: 0o644, Content: ""},
		{Name: "var//log/../lib/./x", Typeflag: tar.TypeReg, Mode: 0o600, Content: "unclean"},
		{Name: "/abs/path", Typeflag: tar.TypeReg, Mode: 0o600, Content: "abs"},
		{Name: "dev/fifo", Typeflag: tar.TypeFifo, Mode: 0o600},
		{Name: "release-manifests/", Typeflag: tar.TypeDir, Mode: 0o755},
		{Name: "release-manifests/image-references", Typeflag: tar.TypeReg, Mode: 0o644, Content: `{"spec":{"tags":[{"name":"pod","from":{"name":"quay.io/x@sha256:old"}}]}}`},
		{Name: "long/" + strings.Repeat("d", 120) + "/" + strings.Repeat("f", 120), Typeflag: tar.TypeReg, Mode: 0o644, Content: "long name"},
	}
	upper := []tarEntry{
		{Name: "etc/", Typeflag: tar.TypeDir, Mode: 0o700},
		{Name: "etc/passwd", Typeflag: tar.TypeReg, Mode: 0o600, Content: "root:x:0:0\nuser:x:1000:1000\n"},
		{Name: "etc/.wh.os-release", Typeflag: tar.TypeReg, Mode: 0o644},
		{Name: "usr/bin/.wh.tool", Typeflag: tar.TypeReg, Mode: 0o644},
		{Name: "usr/bin/tool", Typeflag: tar.TypeReg, Mode: 0o755, Content: "#!/bin/sh\necho upper\n"},
		{Name: "opt/.wh.app", Typeflag: tar.TypeReg, Mode: 0o644},
		{Name: "var/lib/.wh..wh..opq", Typeflag: tar.TypeReg, Mode: 0o644},
		{Name: "release-manifests/image-references", Typeflag: tar.TypeReg, Mode: 0o644, Content: `{"spec":{"tags":[{"name":"pod","from":{"name":"quay.io/x@sha256:new"}}]}}`},
		{Name: "new/", Typeflag: tar.TypeDir, Mode: 0o755},
		{Name: "new/file", Typeflag: tar.TypeReg, Mode: 0o644, Content: "new"},
		{Name: "new/file", Typeflag: tar.TypeReg, Mode: 0o644, Content: "dup in same layer"},
	}
	top := []tarEntry{
		{Name: "new/.wh.file", Typeflag: tar.TypeReg, Mode: 0o644},
		{Name: "opt/app/", Typeflag: tar.TypeDir, Mode: 0o755},
		{Name: "opt/app/again.txt", Typeflag: tar.TypeReg, Mode: 0o644, Content: "again"},
		{Name: "dir-then-file", Typeflag: tar.TypeDir, Mode: 0o755},
	}
	bottomOverride := []tarEntry{
		{Name: "dir-then-file", Typeflag: tar.TypeReg, Mode: 0o644, Content: "file below dir"},
		{Name: "dir-then-file/child", Typeflag: tar.TypeReg, Mode: 0o644, Content: "child"},
	}

	baseTar := buildTar(base)
	upperTar := buildTar(upper)
	topTar := buildTar(top)
	add("single-uncompressed", [][]byte{baseTar})
	add("single-gzip", [][]byte{gz(baseTar)})
	add("single-zstd", [][]byte{zst(baseTar)})
	add("three-layers-mixed", [][]byte{gz(baseTar), zst(upperTar), topTar})
	add("four-layers", [][]byte{gz(buildTar(bottomOverride)), gz(baseTar), gz(upperTar), gz(topTar)})
	add("empty-layer", [][]byte{gz(buildTar(nil)), gz(baseTar)})
	add("pax-format", [][]byte{gz(buildTar(base, func(h *tar.Header) { h.Format = tar.FormatPAX; h.PAXRecords = map[string]string{"comment": "x"} }))})
	add("gnu-format", [][]byte{gz(buildTar(base, func(h *tar.Header) { h.Format = tar.FormatGNU }))})
	// Multi-stream gzip (two gzip members concatenated).
	half := len(baseTar) / 1024 * 512
	add("multistream-gzip", [][]byte{append(gz(baseTar[:half]), gz(baseTar[half:])...)})
	// Truncated layer in the top layer: error at an entry boundary vs mid-entry.
	add("truncated-mid-entry", [][]byte{gz(baseTar), upperTar[:1024+100]})
	add("truncated-at-boundary", [][]byte{gz(baseTar), upperTar[:1024]})
	add("garbage-layer", [][]byte{gz(baseTar), []byte("this is not a tar archive at all, not even close, really")})
	add("corrupt-gzip", [][]byte{gz(baseTar), append([]byte{0x1f, 0x8b}, bytes.Repeat([]byte{0xff}, 64)...)})
	add("empty-blob", [][]byte{gz(baseTar), {}})
	add("partial-header", [][]byte{gz(baseTar), upperTar[:1024+512+100]})
	add("mid-entry-lower-layer", [][]byte{baseTar[:512*4+10], gz(upperTar)})
	add("truncated-padding-gzip", [][]byte{gz(baseTar), gz(upperTar[:1024+100])})

	// Global PAX header entry.
	{
		var buf bytes.Buffer
		tw := tar.NewWriter(&buf)
		must(tw.WriteHeader(&tar.Header{Typeflag: tar.TypeXGlobalHeader, Name: "pax_global_header", PAXRecords: map[string]string{"comment": "global"}}))
		must(tw.WriteHeader(&tar.Header{Typeflag: tar.TypeReg, Name: "g/file", Mode: 0o644, Size: 2, Format: tar.FormatPAX}))
		_, err := tw.Write([]byte("hi"))
		must(err)
		must(tw.Close())
		add("global-header", [][]byte{buf.Bytes()})
	}
	// Legacy typeflag '\x00' (TypeRegA) entries, with and without trailing slash.
	{
		t := buildTar([]tarEntry{
			{Name: "olddir/", Typeflag: tar.TypeDir, Mode: 0o755},
			{Name: "olddir/oldfile", Typeflag: tar.TypeReg, Mode: 0o644, Content: "legacy"},
		}, func(h *tar.Header) { h.Format = tar.FormatUSTAR })
		patchTypeflag(t, 0, 0)
		patchTypeflag(t, 512, 0)
		add("typeflag-rega", [][]byte{t})
	}
	// Header-only type that claims a size.
	{
		t := buildTar([]tarEntry{
			{Name: "a", Typeflag: tar.TypeReg, Mode: 0o644, Content: "first"},
			{Name: "sym", Typeflag: tar.TypeReg, Mode: 0o777, Content: "abc"},
		})
		patchTypeflag(t, 1024, tar.TypeSymlink)
		add("symlink-with-size", [][]byte{t})
	}
	// A zero block followed by a non-zero block is an invalid header.
	{
		a := buildTar([]tarEntry{{Name: "a", Typeflag: tar.TypeReg, Mode: 0o644, Content: "first"}})
		b := buildTar([]tarEntry{{Name: "b", Typeflag: tar.TypeReg, Mode: 0o644, Content: "second"}})
		t := append(append(append([]byte{}, a[:1024]...), make([]byte, 512)...), b...)
		add("zero-block-then-header", [][]byte{gz(baseTar), t})
	}
	// Single zero block then EOF is a clean end.
	{
		a := buildTar([]tarEntry{{Name: "a", Typeflag: tar.TypeReg, Mode: 0o644, Content: "first"}})
		t := append(append([]byte{}, a[:1024]...), make([]byte, 512)...)
		add("single-zero-block", [][]byte{gz(baseTar), t})
	}
	// GNU base-256 numeric fields and long names; PAX overrides.
	add("gnu-base256", [][]byte{buildTar([]tarEntry{
		{Name: "big-ids", Typeflag: tar.TypeReg, Mode: 0o644, Content: "ids"},
		{Name: "dir/" + strings.Repeat("n", 150), Typeflag: tar.TypeSymlink, Linkname: strings.Repeat("l", 150), Mode: 0o777},
	}, func(h *tar.Header) {
		h.Format = tar.FormatGNU
		h.Uid = 1 << 30
		h.Gid = 1 << 31
		h.ModTime = time.Unix(1700000000, 0)
	})})
	add("pax-overrides", [][]byte{buildTar([]tarEntry{
		{Name: "pax-ids", Typeflag: tar.TypeReg, Mode: 0o644, Content: "ids"},
		{Name: "dir/" + strings.Repeat("p", 150), Typeflag: tar.TypeReg, Mode: 0o600, Content: "x"},
	}, func(h *tar.Header) {
		h.Format = tar.FormatPAX
		h.Uid = 1 << 30
		h.Uname = strings.Repeat("u", 40)
		h.ModTime = time.Unix(1700000000, 5)
	})})

	writeJSON(dir, "extract_golden.json", cases)
}

func main() {
	dir := "crates/registry/tests/fixtures"
	if len(os.Args) > 1 {
		dir = os.Args[1]
	}
	must(os.MkdirAll(dir, 0o755))
	genName(dir)
	genAuthn(dir)
	genChallenge(dir)
	genErrors(dir)
	genExtract(dir)
}
