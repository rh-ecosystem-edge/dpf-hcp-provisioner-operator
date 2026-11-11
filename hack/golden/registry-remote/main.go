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

// Command registry-remote runs go-containerregistry's remote.Head/Get/Image (+ConfigFile)
// against scripted httptest registries and records the results / exact error strings,
// so the Rust port (crates/registry) can replay the same scenarios against its fake
// registry and compare.
//
// Usage: go run -mod=vendor ./hack/golden/registry-remote [outfile]
package main

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"strings"
	"time"

	"github.com/google/go-containerregistry/pkg/authn"
	"github.com/google/go-containerregistry/pkg/name"
	"github.com/google/go-containerregistry/pkg/v1/remote"
)

type rule struct {
	Method  string            `json:"method"` // "" matches any
	Path    string            `json:"path"`
	Status  int               `json:"status"`
	Headers map[string]string `json:"headers,omitempty"`
	Body    string            `json:"body,omitempty"`
}

type scenario struct {
	Name     string   `json:"name"`
	Op       string   `json:"op"` // head | get | image | config
	Ref      string   `json:"ref"`
	User     string   `json:"user,omitempty"`
	Pass     string   `json:"pass,omitempty"`
	Dead     bool     `json:"dead,omitempty"` // nothing listening
	Rules    []rule   `json:"rules"`
	Expected string   `json:"expected"`
	Result   string   `json:"result,omitempty"`
	Requests []string `json:"requests,omitempty"`
}

func sha(b string) string {
	s := sha256.Sum256([]byte(b))
	return "sha256:" + hex.EncodeToString(s[:])
}

type keychain struct{ user, pass string }

func (k keychain) Resolve(authn.Resource) (authn.Authenticator, error) {
	if k.user == "" {
		return authn.Anonymous, nil
	}
	return authn.FromConfig(authn.AuthConfig{Username: k.user, Password: k.pass}), nil
}

func run(sc *scenario) {
	var host string
	var reqs []string
	if sc.Dead {
		l, err := net.Listen("tcp", "127.0.0.1:0")
		if err != nil {
			panic(err)
		}
		host = l.Addr().String()
		l.Close()
	} else {
		srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			reqs = append(reqs, r.Method+" "+r.URL.RequestURI())
			for _, ru := range sc.Rules {
				if (ru.Method == "" || ru.Method == r.Method) && ru.Path == r.URL.Path {
					for k, v := range ru.Headers {
						w.Header().Set(k, strings.ReplaceAll(v, "HOST", host))
					}
					w.WriteHeader(ru.Status)
					_, _ = w.Write([]byte(ru.Body))
					return
				}
			}
			w.WriteHeader(404)
		}))
		defer srv.Close()
		host = strings.TrimPrefix(srv.URL, "http://")
	}
	ref, err := name.ParseReference(strings.ReplaceAll(sc.Ref, "HOST", host))
	if err != nil {
		panic(err)
	}
	opts := []remote.Option{
		remote.WithAuthFromKeychain(keychain{sc.User, sc.Pass}),
		remote.WithRetryBackoff(remote.Backoff{Duration: time.Millisecond, Factor: 3, Jitter: 0.1, Steps: 3}),
	}
	switch sc.Op {
	case "head":
		var d interface{ String() string }
		desc, e := remote.Head(ref, opts...)
		err = e
		if e == nil {
			d = desc.Digest
			sc.Result = fmt.Sprintf("%s %s %d", desc.MediaType, d.String(), desc.Size)
		}
	case "get":
		desc, e := remote.Get(ref, opts...)
		err = e
		if e == nil {
			sc.Result = fmt.Sprintf("%s %s %d", desc.MediaType, desc.Digest, desc.Size)
		}
	case "image", "config":
		img, e := remote.Image(ref, opts...)
		err = e
		if e == nil {
			d, _ := img.Digest()
			sc.Result = d.String()
			if sc.Op == "config" {
				cfg, e := img.ConfigFile()
				err = e
				if e == nil {
					sc.Result = fmt.Sprintf("%v", cfg.Config.Labels)
				}
			}
		}
	}
	if err != nil {
		sc.Expected = strings.ReplaceAll(err.Error(), host, "HOST")
	}
	for _, r := range reqs {
		sc.Requests = append(sc.Requests, strings.ReplaceAll(r, host, "HOST"))
	}
}

func main() {
	out := "crates/registry/tests/fixtures/remote_golden.json"
	if len(os.Args) > 1 {
		out = os.Args[1]
	}
	ok := rule{Path: "/v2/", Status: 200, Body: "{}"}
	bearer := rule{Path: "/v2/", Status: 401, Headers: map[string]string{"WWW-Authenticate": `Bearer realm="http://HOST/token",service="svc"`}}
	manifest := `{"schemaVersion":2,"mediaType":"application/vnd.oci.image.manifest.v1+json","config":{"mediaType":"application/vnd.oci.image.config.v1+json","size":53,"digest":"` + sha(`{"config":{"Labels":{"io.openshift.release":"4.19"}}}`) + `"},"layers":[]}`
	cfg := `{"config":{"Labels":{"io.openshift.release":"4.19"}}}`
	ociMT := map[string]string{"Content-Type": "application/vnd.oci.image.manifest.v1+json"}
	index := `{"schemaVersion":2,"mediaType":"application/vnd.oci.image.index.v1+json","manifests":[{"mediaType":"application/vnd.oci.image.manifest.v1+json","size":1,"digest":"` + sha("x") + `","platform":{"architecture":"arm64","os":"linux","variant":"v8"}}]}`
	scenarios := []*scenario{
		{Name: "head-ok", Op: "head", Ref: "HOST/foo/bar:v1", Rules: []rule{ok, {Method: "HEAD", Path: "/v2/foo/bar/manifests/v1", Status: 200, Headers: map[string]string{"Content-Type": "application/vnd.oci.image.index.v1+json", "Docker-Content-Digest": sha("m"), "Content-Length": "123"}}}},
		{Name: "head-404", Op: "head", Ref: "HOST/foo/bar:v1", Rules: []rule{ok}},
		{Name: "get-404-json", Op: "get", Ref: "HOST/foo/bar:v1", Rules: []rule{ok, {Path: "/v2/foo/bar/manifests/v1", Status: 404, Body: `{"errors":[{"code":"MANIFEST_UNKNOWN","message":"manifest unknown","detail":{"Tag":"v1"}}]}`}}},
		{Name: "name-unknown", Op: "get", Ref: "HOST/foo/bar:v1", Rules: []rule{ok, {Path: "/v2/foo/bar/manifests/v1", Status: 404, Body: `{"errors":[{"code":"NAME_UNKNOWN","message":"repository name not known to registry","detail":{"name":"foo/bar"}}]}`}}},
		{Name: "denied", Op: "head", Ref: "HOST/foo/bar:v1", Rules: []rule{ok, {Path: "/v2/foo/bar/manifests/v1", Status: 403, Body: `{"errors":[{"code":"DENIED","message":"requested access to the resource is denied"}]}`}}},
		{Name: "token-401", Op: "head", Ref: "HOST/foo/bar:v1", User: "u", Pass: "p", Rules: []rule{bearer, {Path: "/token", Status: 401, Body: `{"errors":[{"code":"UNAUTHORIZED","message":"access to the requested resource is not authorized","detail":[{"Type":"repository","Name":"foo/bar","Action":"pull"}]}]}`}}},
		{Name: "token-ok-manifest-401", Op: "get", Ref: "HOST/foo/bar:v1", Rules: []rule{bearer, {Path: "/token", Status: 200, Body: `{"token":"t"}`}, {Path: "/v2/foo/bar/manifests/v1", Status: 401, Body: `{"errors":[{"code":"UNAUTHORIZED","message":"authentication required","detail":null}]}`}}},
		{Name: "token-ok-manifest-401-challenge", Op: "head", Ref: "HOST/foo/bar:v1", Rules: []rule{bearer, {Path: "/token", Status: 200, Body: `{"access_token":"t","refresh_token":"r"}`}, {Path: "/v2/foo/bar/manifests/v1", Status: 401, Headers: map[string]string{"WWW-Authenticate": `Bearer realm="http://HOST/token",service="svc",scope="repository:foo/bar:pull,push"`}}}},
		{Name: "basic-401", Op: "head", Ref: "HOST/foo/bar:v1", Rules: []rule{{Path: "/v2/", Status: 401, Headers: map[string]string{"WWW-Authenticate": `Basic realm="r"`}}, {Path: "/v2/foo/bar/manifests/v1", Status: 401}}},
		{Name: "negotiate-401", Op: "get", Ref: "HOST/foo/bar:v1", Rules: []rule{{Path: "/v2/", Status: 401, Headers: map[string]string{"WWW-Authenticate": `Negotiate`}}, {Path: "/v2/foo/bar/manifests/v1", Status: 401, Body: "nope"}}},
		{Name: "missing-realm", Op: "head", Ref: "HOST/foo/bar:v1", Rules: []rule{{Path: "/v2/", Status: 401, Headers: map[string]string{"WWW-Authenticate": `Bearer service="svc",scope="x"`}}}},
		{Name: "no-token", Op: "head", Ref: "HOST/foo/bar:v1", Rules: []rule{bearer, {Path: "/token", Status: 200, Body: `{"expires_in":300}`}}},
		{Name: "token-html", Op: "head", Ref: "HOST/foo/bar:v1", Rules: []rule{bearer, {Path: "/token", Status: 200, Body: `<html>`}}},
		{Name: "token-bad-type", Op: "head", Ref: "HOST/foo/bar:v1", Rules: []rule{bearer, {Path: "/token", Status: 200, Body: `{"token":5}`}}},
		{Name: "head-503", Op: "head", Ref: "HOST/foo/bar:v1", Rules: []rule{ok, {Path: "/v2/foo/bar/manifests/v1", Status: 503, Body: "busy"}}},
		{Name: "get-500-json", Op: "get", Ref: "HOST/foo/bar:v1", Rules: []rule{ok, {Path: "/v2/foo/bar/manifests/v1", Status: 500, Body: `{"errors":[{"code":"UNKNOWN","message":"boom"}]}`}}},
		{Name: "get-429", Op: "get", Ref: "HOST/foo/bar:v1", Rules: []rule{ok, {Path: "/v2/foo/bar/manifests/v1", Status: 429, Body: `{"errors":[{"code":"TOOMANYREQUESTS","message":"slow down"}]}`}}},
		{Name: "ping-404", Op: "head", Ref: "HOST/foo/bar:v1", Rules: []rule{{Path: "/v2/", Status: 404, Body: "no registry here"}}},
		{Name: "ping-502", Op: "head", Ref: "HOST/foo/bar:v1", Rules: []rule{{Path: "/v2/", Status: 502, Body: "bad gateway"}}},
		{Name: "dead", Op: "head", Ref: "HOST/foo/bar:v1", Dead: true},
		{Name: "head-no-digest", Op: "head", Ref: "HOST/foo/bar:v1", Rules: []rule{ok, {Path: "/v2/foo/bar/manifests/v1", Status: 200, Headers: map[string]string{"Content-Type": "application/vnd.oci.image.manifest.v1+json", "Content-Length": "10"}}}},
		{Name: "head-no-content-type", Op: "head", Ref: "HOST/foo/bar:v1", Rules: []rule{ok, {Path: "/v2/foo/bar/manifests/v1", Status: 200, Headers: map[string]string{"Docker-Content-Digest": sha("m"), "Content-Length": "10"}}}},
		{Name: "head-bad-digest", Op: "head", Ref: "HOST/foo/bar:v1", Rules: []rule{ok, {Path: "/v2/foo/bar/manifests/v1", Status: 200, Headers: map[string]string{"Content-Type": "x", "Docker-Content-Digest": "sha256:xyz", "Content-Length": "10"}}}},
		{Name: "digest-mismatch", Op: "get", Ref: "HOST/foo/bar@" + sha("other"), Rules: []rule{ok, {Path: "/v2/foo/bar/manifests/" + sha("other"), Status: 200, Headers: ociMT, Body: manifest}}},
		{Name: "config-ok", Op: "config", Ref: "HOST/foo/bar:v1", Rules: []rule{ok, {Path: "/v2/foo/bar/manifests/v1", Status: 200, Headers: ociMT, Body: manifest}, {Path: "/v2/foo/bar/blobs/" + sha(cfg), Status: 200, Body: cfg}}},
		{Name: "config-tampered", Op: "config", Ref: "HOST/foo/bar:v1", Rules: []rule{ok, {Path: "/v2/foo/bar/manifests/v1", Status: 200, Headers: ociMT, Body: manifest}, {Path: "/v2/foo/bar/blobs/" + sha(cfg), Status: 200, Body: strings.Replace(cfg, "4.19", "4.20", 1)}}},
		{Name: "config-short", Op: "config", Ref: "HOST/foo/bar:v1", Rules: []rule{ok, {Path: "/v2/foo/bar/manifests/v1", Status: 200, Headers: ociMT, Body: manifest}, {Path: "/v2/foo/bar/blobs/" + sha(cfg), Status: 200, Body: cfg[:20]}}},
		{Name: "config-missing", Op: "config", Ref: "HOST/foo/bar:v1", Rules: []rule{ok, {Path: "/v2/foo/bar/manifests/v1", Status: 200, Headers: ociMT, Body: manifest}}},
		{Name: "config-bad-json", Op: "config", Ref: "HOST/foo/bar:v1", Rules: []rule{ok, {Path: "/v2/foo/bar/manifests/v1", Status: 200, Headers: ociMT, Body: strings.Replace(manifest, sha(cfg), sha("<"), 1)}, {Path: "/v2/foo/bar/blobs/" + sha("<"), Status: 200, Body: "<"}}},
		{Name: "index-no-platform", Op: "image", Ref: "HOST/foo/bar:v1", Rules: []rule{ok, {Path: "/v2/foo/bar/manifests/v1", Status: 200, Headers: map[string]string{"Content-Type": "application/vnd.oci.image.index.v1+json"}, Body: index}}},
		{Name: "schema1", Op: "image", Ref: "HOST/foo/bar:v1", Rules: []rule{ok, {Path: "/v2/foo/bar/manifests/v1", Status: 200, Headers: map[string]string{"Content-Type": "application/vnd.docker.distribution.manifest.v1+prettyjws"}, Body: "{}"}}},
		{Name: "redirect-loop", Op: "get", Ref: "HOST/foo/bar:v1", Rules: []rule{ok, {Path: "/v2/foo/bar/manifests/v1", Status: 302, Headers: map[string]string{"Location": "/v2/foo/bar/manifests/v1"}}}},
		{Name: "redirect-ok", Op: "get", Ref: "HOST/foo/bar:v1", Rules: []rule{ok, {Path: "/v2/foo/bar/manifests/v1", Status: 301, Headers: map[string]string{"Location": "/elsewhere?x=1"}}, {Path: "/elsewhere", Status: 200, Headers: ociMT, Body: manifest}}},
		{Name: "redirect-no-location", Op: "get", Ref: "HOST/foo/bar:v1", Rules: []rule{ok, {Path: "/v2/foo/bar/manifests/v1", Status: 302}}},
	}
	for _, sc := range scenarios {
		run(sc)
	}
	b, err := json.MarshalIndent(scenarios, "", "  ")
	if err != nil {
		panic(err)
	}
	if err := os.WriteFile(out, append(b, '\n'), 0o644); err != nil {
		panic(err)
	}
}
