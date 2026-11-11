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

// Command ignition generates the golden fixtures of crates/ignition: it runs representative,
// edge-case and randomized (fixed seed) inputs through every exported function of
// internal/ignition, internal/ignition/content and internal/ignition/resources/* and records
// the results (json.Marshal of the resulting config, MarshalJSON/EncodeIgnition output, error
// strings, validation reports).
//
// Every input config is stored as JSON text and decoded with json.Unmarshal on both sides,
// so Go and Rust start from the same value.
//
// Usage: go run -mod=vendor ./hack/golden/ignition [crates/ignition/tests/fixtures]
package main

import (
	"bytes"
	"compress/gzip"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"math/rand"
	"net/url"
	"os"
	"path/filepath"
	"reflect"
	"strings"

	"github.com/coreos/ignition/v2/config/merge"
	igntypes "github.com/coreos/ignition/v2/config/v3_4/types"
	ignvalidate "github.com/coreos/ignition/v2/config/validate"
	dpuprovisioningv1alpha1 "github.com/nvidia/doca-platform/api/provisioning/v1alpha1"

	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/ignition"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/ignition/content"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/ignition/resources/common"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/ignition/resources/live"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/ignition/resources/target"
)

var outDir = "crates/ignition/tests/fixtures"

func must(err error) {
	if err != nil {
		panic(err)
	}
}

func errStr(err error) string {
	if err == nil {
		return ""
	}
	return err.Error()
}

func raw(cfg *igntypes.Config) string {
	b, err := json.Marshal(cfg)
	must(err)
	return string(b)
}

func decode(s string) *igntypes.Config {
	var cfg igntypes.Config
	must(json.Unmarshal([]byte(s), &cfg))
	return &cfg
}

func write(name string, v any) {
	b, err := json.MarshalIndent(v, "", " ")
	must(err)
	must(os.WriteFile(filepath.Join(outDir, name), append(b, '\n'), 0o644))
	fmt.Printf("wrote %s (%d bytes)\n", name, len(b))
}

func gz(data []byte) []byte {
	var buf bytes.Buffer
	w, _ := gzip.NewWriterLevel(&buf, gzip.BestCompression)
	_, _ = w.Write(data)
	_ = w.Close()
	return buf.Bytes()
}

// ------------------------------------------------------------------------------------------
// Random configs
// ------------------------------------------------------------------------------------------

type gen struct {
	r *rand.Rand
	// small pools give many key collisions (merge, duplicates)
	small bool
}

func (g *gen) pick(pool []string) string { return pool[g.r.Intn(len(pool))] }

var hex128 = strings.Repeat("0123456789abcdef", 8)

var (
	pathPool = []string{"/etc/a", "/etc/a/b", "/etc/b", "/etc/systemd/system/foo.service",
		"/etc/systemd/system/foo.service.d/x.conf", "/etc/../etc/a", "rel/path", "", "/etc/a/",
		"/dev/sda", "/var/lib/x", "/etc/link", "/etc/link/inner", "/"}
	smallPathPool = []string{"/etc/a", "/etc/b", "/etc/c", "/etc/a/b"}
	sourcePool    = []string{"data:,hello", "data:,hello%20world", "data:;base64,aGk=",
		"data:text/plain;charset=utf-8;base64,aGVsbG8=", "data:text/plain,a%0Ab", "data:,{bad}",
		"data:,a b", "data:;base64,!!", "data:;base64,a", "data:,%zz", "data:application/json,{}",
		"http://example.com/x", "https://[::1]:8443/x", "https://h:99999999999/x", "s3://bucket/key?versionId=",
		"s3://bucket/key?versionId=v1", "arn:aws:s3:::bucket/key", "arn:aws:s3:us-east-1:123:accesspoint/ap",
		"arn:aws:ec2:::x/y", "gs://b/o", "tftp://h/f", "ftp://x/y", "", "nodata", "data:foo/bar,x",
		"data:text/plain;charset=\"utf\\\"8\",x", "http://a b/", "http://[1.2.3.4]/", "data:,x#frag",
		"https://user:pa@ss@host/x", "<html>&amp;</html>"}
	unitContentPool = []string{"[Unit]\nDescription=x\n[Service]\nExecStart=/bin/true\n[Install]\nWantedBy=multi-user.target\n",
		"[Unit]\nDescription=no install\n", "[Unit]\nFoo", "[Unit", "[Unit] garbage\nA=b", "",
		"# comment only\n", "[Service]\nExecStart=/bin/sh -c \\\n  'echo hi'\n[Install]\nWantedBy=x\n",
		"[Unit]\nA\nB=c\n", "  [X]\nk = v  \n", "[Install]\r\nWantedBy=a\r\n"}
	unitNamePool = []string{"foo.service", "bar.service", "foo@.service", "foo@bar.service", "baz.timer", "noext", "x.conf", "a@b@c.service"}
	dropinNames  = []string{"x.conf", "y.conf", "bad.txt", ""}
	hashPool     = []string{"sha512-" + hex128, "sha256-" + hex128[:64], "sha256-abc", "md5-abc", "nodash", "sha512-"}
	compPool     = []string{"", "gzip", "bzip2"}
	formatPool   = []string{"ext4", "xfs", "btrfs", "swap", "vfat", "none", "zfs", ""}
	levelPool    = []string{"raid0", "raid1", "linear", "stripe", "raid5", "raid10", "raid7", ""}
	pinPool      = []string{"tpm2", "tang", "sss", "bogus", ""}
	versionPool  = []string{"3.4.0", "3.3.0", "3.4.0-experimental", "x", "", "3.4"}
	guidPool     = []string{"", "01234567-89AB-CDEF-EDCB-A98765432101", "not-a-guid", "0123456789AB-CDEF-EDCB-A98765432101"}
	labelPool    = []string{"lbl", "a:b", strings.Repeat("l", 37), strings.Repeat("m", 17), strings.Repeat("n", 48), ""}
	genericPool  = []string{"", "x", "name", "<b>&amp;</b>", "ünïcødé", "line\u2028sep", "a\"q\\b", "tab\t", "{{.X}}", "/etc/a"}
	proxyPool    = []string{"http://proxy:3128", "https://proxy", "ftp://proxy", "proxy", ":bad", "http://[::1"}
	tangURLPool  = []string{"http://tang", "https://tang", "tang", "", ":x"}
	advPool      = []string{"{\"a\":1}", "not json", "", "[1e999]"}
	intPool      = []int{0, 1, 2, 3, 420, 0o644, 0o755, 0o7777, 0o10000, 0o4755, -1, 2048, 4096, 10}
	smallIntPool = []int{0, 1, 2}
)

func (g *gen) str(field string) string {
	switch field {
	case "Path", "Device", "Target":
		if g.small {
			return g.pick(smallPathPool)
		}
		return g.pick(pathPool)
	case "Source", "HTTPProxy", "HTTPSProxy":
		if field != "Source" {
			return g.pick(proxyPool)
		}
		return g.pick(sourcePool)
	case "Contents":
		return g.pick(unitContentPool)
	case "Hash":
		return g.pick(hashPool)
	case "Compression":
		return g.pick(compPool)
	case "Format":
		return g.pick(formatPool)
	case "Level":
		return g.pick(levelPool)
	case "Pin":
		return g.pick(pinPool)
	case "Version":
		return g.pick(versionPool)
	case "GUID", "TypeGUID":
		return g.pick(guidPool)
	case "Label":
		return g.pick(labelPool)
	case "URL":
		return g.pick(tangURLPool)
	case "Advertisement":
		return g.pick(advPool)
	}
	if g.small {
		return g.pick([]string{"a", "b", "c"})
	}
	return g.pick(genericPool)
}

func (g *gen) fill(v reflect.Value, field, parent string, depth int) {
	switch v.Kind() {
	case reflect.Ptr:
		if g.r.Intn(2) == 0 {
			return
		}
		v.Set(reflect.New(v.Type().Elem()))
		g.fill(v.Elem(), field, parent, depth)
	case reflect.String:
		f := field
		if parent == "Unit" && field == "Name" {
			v.SetString(g.pick(unitNamePool))
			return
		}
		if parent == "Dropin" && field == "Name" {
			v.SetString(g.pick(dropinNames))
			return
		}
		if parent == "Dropin" && field == "Contents" {
			v.SetString(g.pick(unitContentPool))
			return
		}
		if parent == "HTTPHeader" || parent == "PasswdUser" || parent == "PasswdGroup" || parent == "Luks" || parent == "Raid" || parent == "NodeUser" || parent == "NodeGroup" {
			if field == "Name" {
				f = ""
			}
		}
		if field == "Contents" && parent != "Unit" {
			f = ""
		}
		if field == "Path" && parent == "Filesystem" {
			f = "Path"
		}
		v.SetString(g.str(f))
	case reflect.Int:
		if g.small {
			v.SetInt(int64(smallIntPool[g.r.Intn(len(smallIntPool))]))
		} else {
			v.SetInt(int64(intPool[g.r.Intn(len(intPool))]))
		}
	case reflect.Bool:
		v.SetBool(g.r.Intn(2) == 0)
	case reflect.Slice:
		if depth > 6 || g.r.Intn(5) < 2 {
			return
		}
		n := g.r.Intn(4)
		s := reflect.MakeSlice(v.Type(), n, n)
		for i := 0; i < n; i++ {
			g.fill(s.Index(i), field, v.Type().Elem().Name(), depth+1)
		}
		v.Set(s)
	case reflect.Struct:
		t := v.Type()
		for i := 0; i < v.NumField(); i++ {
			f := t.Field(i)
			name := t.Name()
			if f.Anonymous {
				g.fill(v.Field(i), f.Name, name, depth)
				continue
			}
			// leave whole sub-structs empty sometimes
			if f.Type.Kind() == reflect.Struct && g.r.Intn(4) == 0 {
				continue
			}
			g.fill(v.Field(i), f.Name, name, depth+1)
		}
	}
}

// randomConfig returns the JSON text of a random config.
func (g *gen) randomConfig() string {
	var cfg igntypes.Config
	g.fill(reflect.ValueOf(&cfg).Elem(), "", "", 0)
	if g.r.Intn(3) > 0 {
		cfg.Ignition.Version = "3.4.0"
	}
	return raw(&cfg)
}

// storageConfig returns a random config with only storage/systemd/kargs/passwd lists (for merge).
func (g *gen) listsConfig() string {
	var cfg igntypes.Config
	g.fill(reflect.ValueOf(&cfg.Storage).Elem(), "Storage", "Config", 1)
	g.fill(reflect.ValueOf(&cfg.Systemd).Elem(), "Systemd", "Config", 1)
	g.fill(reflect.ValueOf(&cfg.KernelArguments).Elem(), "KernelArguments", "Config", 1)
	g.fill(reflect.ValueOf(&cfg.Passwd).Elem(), "Passwd", "Config", 1)
	if g.r.Intn(2) == 0 {
		g.fill(reflect.ValueOf(&cfg.Ignition).Elem(), "Ignition", "Config", 1)
	}
	cfg.Ignition.Version = "3.4.0"
	return raw(&cfg)
}

// ------------------------------------------------------------------------------------------
// Fixtures
// ------------------------------------------------------------------------------------------

type configCase struct {
	Name       string `json:"name"`
	Input      string `json:"input"`
	Raw        string `json:"raw"`
	Marshal    string `json:"marshal"`
	Encode     string `json:"encode,omitempty"`
	Gzip       string `json:"gzip"`
	GzipErr    string `json:"gzip_err"`
	Validate   string `json:"validate"`
	Report     string `json:"report"`
	ReportFull string `json:"report_full"`
}

func runConfig(name, input string, withEncode bool) configCase {
	c := configCase{Name: name, Input: input}
	cfg := decode(input)
	c.Raw = raw(cfg)
	m, err := ignition.MarshalJSON(cfg)
	must(err)
	c.Marshal = string(m)
	if withEncode {
		e, err := ignition.EncodeIgnition(cfg)
		must(err)
		c.Encode = e
	}
	c.Validate = errStr(ignition.ValidateConfig(decode(input)))
	cfg2 := decode(input)
	rawBytes, _ := json.Marshal(cfg2)
	rpt := ignvalidate.ValidateWithContext(*cfg2, rawBytes)
	c.Report = rpt.String()
	full := ""
	for _, e := range rpt.Entries {
		full += fmt.Sprintf("%d|%s|%s\n", e.Kind, e.Context.String(), e.Message)
	}
	c.ReportFull = full
	cfg3 := decode(input)
	c.GzipErr = errStr(ignition.GzipIgnitionFiles(cfg3))
	c.Gzip = raw(cfg3)
	return c
}

var handConfigs = []struct{ name, json string }{
	{"empty", `{"ignition":{"version":"3.4.0"}}`},
	{"empty_version", `{}`},
	{"file_simple", `{"ignition":{"version":"3.4.0"},"storage":{"files":[{"path":"/etc/test","contents":{"source":"data:,hello"}}]}}`},
	{"file_overwrite_mode", `{"ignition":{"version":"3.4.0"},"storage":{"files":[{"path":"/etc/test-file","overwrite":true,"mode":420,"contents":{"source":"data:,hello"}}]}}`},
	{"dup_files", `{"ignition":{"version":"3.4.0"},"storage":{"files":[{"path":"/etc/d","overwrite":true,"mode":420,"contents":{"source":"data:,c"}},{"path":"/etc/d","overwrite":true,"mode":420,"contents":{"source":"data:,c"}}]}}`},
	{"dup_units", `{"ignition":{"version":"3.4.0"},"systemd":{"units":[{"name":"d.service","enabled":true,"contents":"[Unit]\nDescription=Test\n[Service]\nExecStart=/bin/true\n[Install]\nWantedBy=multi-user.target"},{"name":"d.service","enabled":true,"contents":"[Unit]\nDescription=Test\n[Service]\nExecStart=/bin/true\n[Install]\nWantedBy=multi-user.target"}]}}`},
	{"file_link_dir_same_path", `{"ignition":{"version":"3.4.0"},"storage":{"files":[{"path":"/etc/x","contents":{"source":"data:,a"}}],"links":[{"path":"/etc/x","target":"/y"}],"directories":[{"path":"/etc/x"}]}}`},
	{"path_conflicts_systemd", `{"ignition":{"version":"3.4.0"},"storage":{"files":[{"path":"/etc/systemd/system/a.service","contents":{"source":"data:,a"}},{"path":"/etc/systemd/system/a.service.d/b.conf"}],"links":[{"path":"/etc/systemd/system/a.service","target":"/x"}],"directories":[{"path":"/etc/systemd/system/a.service.d/b.conf"}]},"systemd":{"units":[{"name":"a.service","contents":"[Unit]\n","dropins":[{"name":"b.conf","contents":"[Unit]\n"}]}]}}`},
	{"html_chars", `{"ignition":{"version":"3.4.0"},"storage":{"files":[{"path":"/etc/<a>&b","contents":{"source":"data:,<x>&\u2028"}}]}}`},
	{"overwrite_nil_source", `{"ignition":{"version":"3.4.0"},"storage":{"files":[{"path":"/etc/a","overwrite":true}]}}`},
	{"bad_modes", `{"ignition":{"version":"3.4.0"},"storage":{"files":[{"path":"/etc/a","mode":4096},{"path":"/etc/b","mode":-1},{"path":"/etc/c","mode":2541}],"directories":[{"path":"/etc/d","mode":1023}]}}`},
	{"bad_paths", `{"ignition":{"version":"3.4.0"},"storage":{"files":[{"path":""},{"path":"rel"},{"path":"/etc/../x"},{"path":"/etc/x/"}]}}`},
	{"links", `{"ignition":{"version":"3.4.0"},"storage":{"links":[{"path":"/etc/l"},{"path":"/etc/l/m","target":"x"},{"path":"/etc/h","hard":true,"target":"d","user":{"id":1,"name":"u"},"group":{"id":2,"name":"g"}}],"directories":[{"path":"/etc/h/d"},{"path":"/etc/l/sub"}],"files":[{"path":"/etc/l/f"}]}}`},
	{"hard_link_abs", `{"ignition":{"version":"3.4.0"},"storage":{"links":[{"path":"/etc/h","hard":true,"target":"/etc/dir/../dir"}],"directories":[{"path":"/etc/dir"}]}}`},
	{"resource_rules", `{"ignition":{"version":"3.4.0"},"storage":{"files":[{"path":"/etc/a","contents":{"compression":"xz","verification":{"hash":"sha512-abc"}}},{"path":"/etc/b","contents":{"source":"data:,x","httpHeaders":[{"name":"","value":""},{"name":"h"}]}},{"path":"/etc/c","contents":{"source":"https://x/y","httpHeaders":[{"name":"A","value":"b"},{"name":"A","value":"c"}],"verification":{"hash":"sha256-` + hex128[:64] + `"}}}],"directories":[]}}`},
	{"ignition_section", `{"ignition":{"version":"3.4.0","config":{"merge":[{"source":""},{"source":"http://x"},{"source":"ftp://x"},{"source":"http://x"}],"replace":{"source":"data:,{"}},"proxy":{"httpProxy":"ftp://p","httpsProxy":"http://p","noProxy":["a","a"]},"security":{"tls":{"certificateAuthorities":[{"source":"data:,x"},{}]}},"timeouts":{"httpTotal":5}}}`},
	{"disks", `{"ignition":{"version":"3.4.0"},"storage":{"disks":[{"device":""},{"device":"/dev/sda","partitions":[{"number":1,"startMiB":1,"sizeMiB":10},{"number":1,"label":"a","startMiB":5,"sizeMiB":10},{"label":"a","shouldExist":false},{"guid":"bad","typeGuid":"01234567-89AB-CDEF-EDCB-A98765432101"},{"label":"x:y"}]},{"device":"/dev/sdb","wipeTable":true},{"device":"/dev/sdb"}],"filesystems":[{"device":"/dev/sda","format":"ext4"},{"device":"/dev/sdb","format":"xfs","label":"` + strings.Repeat("l", 13) + `"},{"device":"/dev/sdc","path":"/x","label":"y"},{"device":"rel","format":"zfs","path":"relp"}]}}`},
	{"luks_raid", `{"ignition":{"version":"3.4.0"},"storage":{"luks":[{"name":"a/b","label":"` + strings.Repeat("x", 48) + `","clevis":{"custom":{"pin":"tpm2","config":"c"},"tpm2":true}},{"name":"c","device":"/dev/x","keyFile":{"source":"ftp://k"},"clevis":{"custom":{"needsNetwork":true},"tang":[{"url":"ftp://t"},{"url":"http://t","thumbprint":"p","advertisement":"{bad"}]}},{"name":"c","device":"rel"}],"raid":[{"name":"r","level":"raid0","spares":1,"devices":["/dev/a","rel"]},{"name":"r","level":"bogus"},{"name":"s"}]}}`},
	{"systemd_rules", `{"ignition":{"version":"3.4.0"},"systemd":{"units":[{"name":"noext","contents":"[Unit"},{"name":"a.service","enabled":true,"contents":"[Unit]\nDescription=x\n"},{"name":"t@.service","contents":"[Unit]\nDescription=t\n"},{"name":"t@x.service","enabled":true},{"name":"u@.service","contents":"[Unit]\n[Install]\nWantedBy=x\n"},{"name":"u@y.service","enabled":true},{"name":"d.service","dropins":[{"name":"a.txt","contents":"bad"},{"name":"b.conf"},{"name":"b.conf"}]}]}}`},
	{"passwd", `{"ignition":{"version":"3.4.0"},"passwd":{"users":[{"name":"core","sshAuthorizedKeys":["k1","k1"],"groups":["g","g"]},{"name":"core"}],"groups":[{"name":"g"},{"name":"g"}]}}`},
	{"kargs", `{"ignition":{"version":"3.4.0"},"kernelArguments":{"shouldExist":["a","b","a"],"shouldNotExist":["b","c"]}}`},
	{"node_owner", `{"ignition":{"version":"3.4.0"},"storage":{"files":[{"path":"/etc/a","user":{"id":1,"name":"x"},"group":{"id":1,"name":""}}],"directories":[{"path":"/etc/b","user":{"name":"x"},"group":{"id":0,"name":"y"}}]}}`},
	{"version_old", `{"ignition":{"version":"3.2.0"},"storage":{"files":[{"path":"/etc/a","contents":{"source":"data:,x"}}]}}`},
	{"version_bad", `{"ignition":{"version":"not-a-version"}}`},
	{"version_two", `{"ignition":{"version":"2.0.0"}}`},
	{"version_pre", `{"ignition":{"version":"3.4.0-experimental"}}`},
	{"empty_lists", `{"ignition":{"version":"3.4.0"},"storage":{"files":[],"links":[]},"systemd":{"units":[]},"passwd":{}}`},
	{"partition_overlap_zero", `{"ignition":{"version":"3.4.0"},"storage":{"disks":[{"device":"/dev/vda","partitions":[{"number":1,"startMiB":0,"sizeMiB":0},{"number":2,"startMiB":10,"sizeMiB":0},{"number":3,"startMiB":10,"sizeMiB":5},{"number":4,"startMiB":1,"sizeMiB":100,"shouldExist":false}]}]}}`},
	{"unit_long_line", `{"ignition":{"version":"3.4.0"},"systemd":{"units":[{"name":"l.service","contents":"[Unit]\nA=` + strings.Repeat("x", 5000) + `\n"},{"name":"m.service","contents":"[Unit]\nA=` + strings.Repeat("y", 2000) + `\nB=` + strings.Repeat("z", 3000) + `"},{"name":"n.service","contents":"` + strings.Repeat("#", 2100) + `\n[Unit]\n"}]}}`},
}

func configFixtures() {
	var cases []configCase
	for _, h := range handConfigs {
		cases = append(cases, runConfig(h.name, h.json, true))
	}
	g := &gen{r: rand.New(rand.NewSource(1))}
	for i := 0; i < 250; i++ {
		cases = append(cases, runConfig(fmt.Sprintf("random_%d", i), g.randomConfig(), i < 20))
	}
	// one-field probes of the URL, data URL, unit parser and hash validators
	for i, s := range sourcePool {
		cfg := ignition.NewEmptyIgnition("3.4.0")
		src := s
		cfg.Storage.Files = []igntypes.File{{Node: igntypes.Node{Path: "/etc/f"}, FileEmbedded1: igntypes.FileEmbedded1{Contents: igntypes.Resource{Source: &src}}}}
		cases = append(cases, runConfig(fmt.Sprintf("source_%d", i), raw(cfg), false))
	}
	extraSources := []string{
		"data:x-foo/bar,a", "data:X-,a", "data:x", "data:text", "data:text/", "data:text/plain", "data:text/plain;",
		"data:text/plain;a", "data:text/plain;a=", "data:text/plain;a=b", "data:text/plain;a=b;", "data:text/plain;a=\"b",
		"data:text/plain;a=\"b\\", "data:text/plain;a=\"b\\\"c\",x", "data:text/plain;a=\"\\x41\",x", "data:text/plain;a=\"\\q\",x",
		"data:text/plain;a=\"\\101\",x", "data:text/plain;a=\"\\400\",x", "data:text/plain;a=\"\\u00e9\",x",
		"data:text/plain;a=\"\\ud800\",x", "data:text/plain;a=\"é\",x", "data:text/plain;a=b%zz,x", "data:text/plain;a=b%4,x",
		"data:text/plain;a=%41,x", "data:text/plain;base64,aGk=", "data:text/plain;base65,aGk=", "data:;base64,aGk=\naGk=",
		"data:;base64,aG k=", "data:,%", "data:,%4", "data:,%41", "data:,é", "data:,a|b", "data:,a^b", "data:,`",
		"data:multipart/x,a", "data:message/x,a", "data:foo,a", "data:\x01,a", "data: ,a", "data:,\t", "DATA:,x",
		"data:text/plain;=a,x", "data:text/plain;,x", "data:text/plain;a=b=c,x", "data:text/(plain),x",
		"http://h:80", "http://h:", "http://h:x", "http://h::1", "http://[::1%25eth0]/", "http://[::1%eth0]/",
		"http://[fe80::1%25%41]/", "http://[::ffff:1.2.3.4]/", "http://[1::2::3]/", "http://[::1]x/", "http://[::1/",
		"http://a[b]/", "http://%41/", "http://%zz/", "http://h/%zz", "http://u%zz@h/", "http://u:p%2@h/", "http://u{x}@h/",
		"http://h<>/", "http://h/?q=%zz", "s3://b/k?versionId", "s3://b/k?versionId=a&versionId=", "s3://b/k?versionId=;",
		"s3://b/k?a=1;versionId=", "arn:aws:s3:::b", "arn:aws:s3:::b/k?versionId=", "arn:aws:s3:::accesspoint/x",
		"arn:aws:s3:::accesspoint/x/y", "arn:a:b", "arn:aws:s3:r:a:accesspoint/ap/object/k", "HTTP://x", "Https://x",
		"1http://x", "+x://y", "a:b:c", "//host/path", "///p", "*", "?q", "#f", "data:,a#%zz", "http://h#a%20b",
		"\x7f", "x\x00y",
	}
	for i, s := range extraSources {
		cfg := ignition.NewEmptyIgnition("3.4.0")
		src := s
		cfg.Storage.Files = []igntypes.File{{Node: igntypes.Node{Path: "/etc/f"}, FileEmbedded1: igntypes.FileEmbedded1{Contents: igntypes.Resource{Source: &src}}}}
		cases = append(cases, runConfig(fmt.Sprintf("source_extra_%d", i), raw(cfg), false))
		// as http-header carrying resource, tang URL and proxy
		hv := "v"
		cfg2 := ignition.NewEmptyIgnition("3.4.0")
		cfg2.Ignition.Config.Replace = igntypes.Resource{Source: &src, HTTPHeaders: igntypes.HTTPHeaders{{Name: "a", Value: &hv}}}
		cfg2.Ignition.Proxy.HTTPSProxy = &src
		cfg2.Storage.Luks = []igntypes.Luks{{Name: "l", Device: ignition.Ptr("/dev/x"), Clevis: igntypes.Clevis{Tang: []igntypes.Tang{{URL: s, Thumbprint: &hv}}}}}
		cases = append(cases, runConfig(fmt.Sprintf("source_extra_ctx_%d", i), raw(cfg2), false))
	}
	units := append([]string{}, unitContentPool...)
	units = append(units, "[A]\n[B]\n", "[A]\n;c\nk=v\\\n  more\n", "[A]\nk=v\\", "[A]\nk=\\\n\\\n", "[A]\n#x \\\n y\nk=v",
		"[A]\n=v\n", "[A]\nk\r=v", "[A]x\n", "[A] \t\n", "x[A]\nk=v", "\ufeff[A]\nk=v", "[A]\n\u00a0k=v\n",
		"[A]\nk=v\n\n\n[B]", "[Install]", "[A]\n"+strings.Repeat("k", 2047)+"=v\n", "[A]\n"+strings.Repeat("k", 4095)+"=v",
		strings.Repeat("\n", 5000)+"[A]\nk=v", "[A]\nk="+strings.Repeat("é", 1500)+"\n", "[A]\nk=v \\ \nm=n")
	for i, u := range units {
		cfg := ignition.NewEmptyIgnition("3.4.0")
		c := u
		cfg.Systemd.Units = []igntypes.Unit{{Name: "u.service", Enabled: ignition.Ptr(true), Contents: &c, Dropins: []igntypes.Dropin{{Name: "d.conf", Contents: &c}}}}
		cases = append(cases, runConfig(fmt.Sprintf("unit_%d", i), raw(cfg), false))
	}
	write("configs.json", cases)
}

// ------------------------------------------------------------------------------------------

type decodeCase struct {
	Input string `json:"input"`
	Raw   string `json:"raw"`
	Err   string `json:"err"`
}

func decodeFixtures() {
	inputs := []string{
		`{}`, `null`, `[]`, `"x"`, `1`, `true`, ``, `{`, `{"ignition":1}`, `{"ignition":{"version":1}}`,
		`{"ignition":{"version":"3.4.0"},"storage":{"files":{}}}`, `{"storage":{"files":[1]}}`,
		`{"storage":{"files":[{"path":1}]}}`, `{"storage":{"files":[{"mode":"x"}]}}`, `{"storage":{"files":[{"mode":1.5}]}}`,
		`{"storage":{"files":[{"mode":1e2}]}}`, `{"storage":{"files":[{"mode":99999999999999999999}]}}`,
		`{"storage":{"files":[{"mode":-0}]}}`, `{"storage":{"files":[{"overwrite":"true"}]}}`,
		`{"storage":{"files":[{"contents":"x"}]}}`, `{"storage":{"files":[{"contents":{"httpHeaders":{}}}]}}`,
		`{"storage":{"files":[{"contents":{"httpHeaders":[{"name":1}]}}]}}`, `{"storage":{"files":[{"append":[{"source":true}]}]}}`,
		`{"storage":{"files":[{"user":{"id":"1"}}]}}`, `{"storage":{"files":[{"group":[]}]}}`,
		`{"storage":{"disks":[{"partitions":[{"number":"1"}]}]}}`, `{"storage":{"disks":[{"partitions":[{"number":1.0}]}]}}`,
		`{"kernelArguments":{"shouldExist":[1]}}`, `{"passwd":{"users":[{"sshAuthorizedKeys":"k"}]}}`,
		`{"systemd":{"units":[{"enabled":1}]}}`, `{"systemd":{"units":[5]}}`, `{"ignition":{"timeouts":{"httpTotal":"5"}}}`,
		`{"ignition":{"proxy":{"noProxy":[{}]}}}`, `{"luks":[1],"storage":{"luks":[{"clevis":{"tang":[{"url":1}]}}]}}`,
		`{"IGNITION":{"VERSION":"3.4.0"},"Storage":{"FILES":[{"PATH":"/a","Contents":{"SOURCE":"data:,x"}}]}}`,
		`{"ignition":{"version":"3.4.0"},"\u017ftorage":{"files":[{"path":"/k"}]}}`,
		`{"ignition":{"version":"a"},"ignition":{"version":"b"}}`,
		`{"storage":{"files":[{"path":"/a","mode":1}]},"storage":{"links":[{"path":"/b"}]}}`,
		`{"storage":{"files":[{"path":"/a","mode":1},{"path":"/b"}]},"storage":{"files":[{"overwrite":true}]}}`,
		`{"storage":{"files":[{"path":"/a","mode":1}]},"storage":{"files":null}}`,
		`{"storage":{"files":[{"path":"/a","mode":1,"overwrite":null,"contents":null}]}}`,
		`{"unknown":1,"storage":{"files":[{"path":"/a","bogus":{"x":[1,2,{"y":null}]}}]}}`,
		`{"ignition":{"version":"3.4.0"},"storage":{"files":[{"path":"/a\u0000b\ud800","contents":{"source":"\u003c\u2028"}}]}}`,
		`{"storage":{"files":[{"path":"x"},{"path":1},{"path":true}]}}`,
		`{"ignition":{"config":{"replace":{"verification":{"hash":5}}}}}`,
		`{"ignition":{"security":{"tls":{"certificateAuthorities":[{"httpHeaders":[{"value":[]}]}]}}}}`,
		` {"ignition" : {"version" : "3.4.0"} , "storage" : { "files" : [ { "path" : "/x" } ] } } `,
		`{"storage":{"files":[{"path":"/x"}]}} x`, `{"a":01}`, `{"a":"\x"}`,
	}
	var cases []decodeCase
	for _, in := range inputs {
		var cfg igntypes.Config
		err := json.Unmarshal([]byte(in), &cfg)
		c := decodeCase{Input: in, Err: errStr(err)}
		if err == nil {
			c.Raw = raw(&cfg)
		}
		cases = append(cases, c)
	}
	write("unmarshal.json", cases)

	// DecodeIgnition
	encs := []string{
		"not-valid-base64!!!", base64.StdEncoding.EncodeToString([]byte("this is not gzip")), "",
		base64.StdEncoding.EncodeToString(gz([]byte(`{"ignition":{"version":"3.4.0"},"storage":{"files":[{"path":"/etc/test"}]}}`))),
		base64.StdEncoding.EncodeToString(gz([]byte(`{bad`))), base64.StdEncoding.EncodeToString(gz([]byte(`[]`))),
		base64.StdEncoding.EncodeToString(gz([]byte(`{"storage":{"files":[{"path":1}]}}`))),
		base64.StdEncoding.EncodeToString(gz([]byte(`{"x":1}`))[:15]),
		base64.StdEncoding.EncodeToString(append(gz([]byte(`{}`))[:20], 0, 0, 0, 0, 0, 0, 0, 0)),
		"aGk", "aGk=\n",
	}
	var dcases []decodeCase
	for _, e := range encs {
		cfg, err := ignition.DecodeIgnition(e)
		c := decodeCase{Input: e, Err: errStr(err)}
		if err == nil {
			c.Raw = raw(cfg)
		}
		dcases = append(dcases, c)
	}
	write("decode_ignition.json", dcases)
}

// ------------------------------------------------------------------------------------------

type mergeCase struct {
	Parent string `json:"parent"`
	Child  string `json:"child"`
	Result string `json:"result"`
}

type flavorMergeCase struct {
	Name   string `json:"name"`
	Base   string `json:"base"`
	Spec   string `json:"spec"`
	Result string `json:"result"`
	Err    string `json:"err"`
}

func mergeFixtures() {
	var cases []mergeCase
	hand := [][2]string{
		{`{"ignition":{"version":"3.4.0"}}`, `{"ignition":{"version":"3.2.0"}}`},
		{`{"ignition":{"version":"3.4.0","config":{"replace":{"source":"data:,a","compression":"gzip"}}}}`, `{"ignition":{"version":"3.4.0","config":{"replace":{"source":"data:,b"}}}}`},
		{`{"ignition":{"version":"3.4.0"},"storage":{"files":[{"path":"/a","contents":{"source":"data:,p","httpHeaders":[{"name":"x","value":"1"},{"name":"y","value":"2"}]}}]}}`, `{"ignition":{"version":"3.4.0"},"storage":{"files":[{"path":"/a","contents":{"httpHeaders":[{"name":"x"},{"name":"z","value":"3"}]}}]}}`},
		{`{"ignition":{"version":"3.4.0"},"storage":{"files":[{"path":"/a"}],"links":[{"path":"/b","target":"/t"}],"directories":[{"path":"/c"}]}}`, `{"ignition":{"version":"3.4.0"},"storage":{"links":[{"path":"/a","target":"/x"}],"files":[{"path":"/b"}]}}`},
		{`{"ignition":{"version":"3.4.0"},"storage":{"files":[{"path":"/a","append":[{"source":"data:,1"}]}]}}`, `{"ignition":{"version":"3.4.0"},"storage":{"files":[{"path":"/a","append":[{"source":"data:,1"}],"mode":384}]}}`},
		{`{"ignition":{"version":"3.4.0"},"kernelArguments":{"shouldExist":["a","b"],"shouldNotExist":["c"]}}`, `{"ignition":{"version":"3.4.0"},"kernelArguments":{"shouldNotExist":["a"],"shouldExist":["c","d"]}}`},
		{`{"ignition":{"version":"3.4.0"},"storage":{"disks":[{"device":"/d","partitions":[{"number":1,"label":"x"},{"label":"y","sizeMiB":5}]}]}}`, `{"ignition":{"version":"3.4.0"},"storage":{"disks":[{"device":"/d","wipeTable":true,"partitions":[{"label":"y","startMiB":1},{"number":1,"label":"z"}]}]}}`},
		{`{"ignition":{"version":"3.4.0"},"systemd":{"units":[{"name":"a.service","enabled":true,"dropins":[{"name":"x.conf","contents":"1"}]}]}}`, `{"ignition":{"version":"3.4.0"},"systemd":{"units":[{"name":"a.service","mask":true,"dropins":[{"name":"x.conf"},{"name":"y.conf"}]},{"name":"b.service"}]}}`},
		{`{"ignition":{"version":"3.4.0"},"storage":{"files":[{"path":"/a"},{"path":"/a","mode":1}]}}`, `{"ignition":{"version":"3.4.0"},"storage":{"files":[{"path":"/a","mode":2},{"path":"/a","mode":3}]}}`},
		{`{"ignition":{"version":"3.4.0"},"storage":{"files":[{"path":"/a"}]}}`, `{"ignition":{"version":"3.4.0"},"storage":{"files":[],"links":[{"path":"/a"}]}}`},
		{`{"ignition":{"version":"3.4.0"},"storage":{"luks":[{"name":"l","clevis":{"tang":[{"url":"http://a","thumbprint":"1"}],"custom":{"pin":"tpm2"}},"options":["a","a"],"openOptions":["x"]}]}}`, `{"ignition":{"version":"3.4.0"},"storage":{"luks":[{"name":"l","clevis":{"tang":[{"url":"http://a","advertisement":"{}"},{"url":"http://b"}]},"options":["a"],"openOptions":["x","y"]}]}}`},
	}
	for _, h := range hand {
		res, _ := merge.MergeStructTranscribe(*decode(h[0]), *decode(h[1]))
		r := res.(igntypes.Config)
		cases = append(cases, mergeCase{h[0], h[1], raw(&r)})
	}
	g := &gen{r: rand.New(rand.NewSource(2)), small: true}
	for i := 0; i < 300; i++ {
		p, c := g.listsConfig(), g.listsConfig()
		res, _ := merge.MergeStructTranscribe(*decode(p), *decode(c))
		r := res.(igntypes.Config)
		cases = append(cases, mergeCase{p, c, raw(&r)})
	}
	write("merge.json", cases)

	// MergeFlavorConfigFiles
	defRes, _ := ignition.EncodeGzipFile([]byte("DEFAULT_CONTENT"))
	base := ignition.NewEmptyIgnition("3.4.0")
	base.Storage.Files = []igntypes.File{
		{Node: igntypes.Node{Path: "/etc/mellanox/mlnx-bf.conf", Overwrite: ignition.Ptr(true)}, FileEmbedded1: igntypes.FileEmbedded1{Contents: defRes, Mode: ignition.Ptr(0644)}},
		{Node: igntypes.Node{Path: "/etc/mellanox/mlnx-ovs.conf", Overwrite: ignition.Ptr(true)}, FileEmbedded1: igntypes.FileEmbedded1{Contents: defRes, Mode: ignition.Ptr(0644)}},
		{Node: igntypes.Node{Path: "/usr/local/bin/install-dpu-agent.sh", Overwrite: ignition.Ptr(true)}, FileEmbedded1: igntypes.FileEmbedded1{Contents: defRes, Mode: ignition.Ptr(0755)}},
	}
	base.Storage.Links = []igntypes.Link{{Node: igntypes.Node{Path: "/etc/link"}, LinkEmbedded1: igntypes.LinkEmbedded1{Target: ignition.Ptr("/x")}}}
	base.Systemd.Units = []igntypes.Unit{{Name: "a.service", Enabled: ignition.Ptr(true)}}
	baseJSON := raw(base)
	specs := map[string]string{
		"none":           `{}`,
		"override":       `{"configFiles":[{"path":"/etc/mellanox/mlnx-bf.conf","operation":"override","raw":"ALLOW_SHARED_RQ=\"yes\"\n","permissions":"0644"}]}`,
		"append":         `{"configFiles":[{"path":"/etc/mellanox/mlnx-bf.conf","operation":"append","raw":"EXTRA\n","permissions":"0600"}]}`,
		"new":            `{"configFiles":[{"path":"/etc/new.conf","raw":"N"}]}`,
		"mix":            `{"configFiles":[{"path":"/etc/mellanox/mlnx-bf.conf","operation":"override","raw":"R"},{"path":"/etc/mellanox/mlnx-ovs.conf","operation":"append","raw":"A"},{"path":"/etc/dpf/custom.conf","operation":"override","raw":"C"}]}`,
		"invalid_perm":   `{"configFiles":[{"path":"/etc/x","raw":"x","permissions":"invalid"}]}`,
		"octal_digit":    `{"configFiles":[{"path":"/etc/x","raw":"x","permissions":"0999"}]}`,
		"perm_range":     `{"configFiles":[{"path":"/etc/x","raw":"x","permissions":"777777777777"}]}`,
		"perm_neg":       `{"configFiles":[{"path":"/etc/x","raw":"x","permissions":"-17"}]}`,
		"no_raw":         `{"configFiles":[{"path":"/etc/ok","raw":"ok"},{"path":"/etc/x"}]}`,
		"empty_path":     `{"configFiles":[{"path":"","raw":"skip"},{"path":"/etc/v","raw":"v"}]}`,
		"empty_raw":      `{"configFiles":[{"path":"/etc/e","raw":""}]}`,
		"twice":          `{"configFiles":[{"path":"/etc/t","raw":"1"},{"path":"/etc/t","operation":"append","raw":"2"},{"path":"/etc/t","raw":"3","permissions":"755"}]}`,
		"over_link":      `{"configFiles":[{"path":"/etc/link","raw":"L"}]}`,
		"append_new":     `{"configFiles":[{"path":"/etc/appended","operation":"append","raw":"A"}]}`,
		"unknown_op":     `{"configFiles":[{"path":"/usr/local/bin/install-dpu-agent.sh","operation":"bogus","raw":"#!/bin/sh\n<&>"}]}`,
		"content_from":   `{"configFiles":[{"path":"/etc/cm","contentFrom":{"configMapKeyRef":{"name":"n","key":"k"}}}]}`,
		"multi_override": `{"configFiles":[{"path":"/etc/mellanox/mlnx-bf.conf","raw":"NEW_BF\n"},{"path":"/etc/mellanox/mlnx-ovs.conf","raw":"NEW_OVS\n"}]}`,
	}
	var fcases []flavorMergeCase
	for _, name := range sortedKeys(specs) {
		var spec dpuprovisioningv1alpha1.DPUFlavorSpec
		must(json.Unmarshal([]byte(specs[name]), &spec))
		b := decode(baseJSON)
		err := ignition.MergeFlavorConfigFiles(b, &spec)
		fcases = append(fcases, flavorMergeCase{Name: name, Base: baseJSON, Spec: specs[name], Result: raw(b), Err: errStr(err)})
	}
	write("merge_flavor.json", fcases)
}

func sortedKeys(m map[string]string) []string {
	var ks []string
	for k := range m {
		ks = append(ks, k)
	}
	for i := range ks {
		for j := i + 1; j < len(ks); j++ {
			if ks[j] < ks[i] {
				ks[i], ks[j] = ks[j], ks[i]
			}
		}
	}
	return ks
}

// ------------------------------------------------------------------------------------------

type opCase struct {
	Name   string `json:"name"`
	Input  string `json:"input"`
	Arg    string `json:"arg"`
	Result string `json:"result"`
	Err    string `json:"err"`
}

func mcSource(v any) string {
	b, err := json.Marshal(v)
	must(err)
	return "data:," + url.QueryEscape(string(b))
}

func opFixtures() {
	const mcPath = "/etc/ignition-machine-config-encapsulated.json"
	mk := func(files ...igntypes.File) string {
		c := ignition.NewEmptyIgnition("3.4.0")
		c.Storage.Files = files
		return raw(c)
	}
	f := func(path string, src *string) igntypes.File {
		return igntypes.File{Node: igntypes.Node{Path: path}, FileEmbedded1: igntypes.FileEmbedded1{Contents: igntypes.Resource{Source: src}}}
	}
	s := func(v string) *string { return &v }
	mcInputs := map[string]string{
		"ok":           mk(f("/etc/other", s("data:,x")), f(mcPath, s(mcSource(map[string]any{"kind": "MachineConfig", "spec": map[string]any{"osImageURL": "old", "n": 1e21, "f": 1.5, "i": 12345678901234567, "h": "<&>"}})))),
		"no_file":      mk(f("/etc/other", s("data:,x"))),
		"empty":        mk(),
		"nil_source":   mk(f(mcPath, nil)),
		"bad_prefix":   mk(f(mcPath, s("not-a-data-uri"))),
		"bad_escape":   mk(f(mcPath, s("data:,%zz"))),
		"bad_json":     mk(f(mcPath, s("data:,"+url.QueryEscape("not-json")))),
		"null_json":    mk(f(mcPath, s("data:,null"))),
		"array_json":   mk(f(mcPath, s("data:,[1]"))),
		"no_spec":      mk(f(mcPath, s(mcSource(map[string]any{"kind": "x"})))),
		"spec_string":  mk(f(mcPath, s(mcSource(map[string]any{"spec": "x"})))),
		"spec_null":    mk(f(mcPath, s("data:,"+url.QueryEscape(`{"spec":null}`)))),
		"two_files":    mk(f(mcPath, s(mcSource(map[string]any{"spec": map[string]any{}}))), f(mcPath, s(mcSource(map[string]any{"spec": map[string]any{"a": 1}})))),
		"plus_spaces":  mk(f(mcPath, s("data:,{\"spec\":{\"x\":\"a+b c\"}}"))),
		"big_number":   mk(f(mcPath, s("data:,"+url.QueryEscape(`{"spec":{"x":1e999}}`)))),
		"invalid_utf8": mk(f(mcPath, s("data:,"+url.QueryEscape("{\"spec\":{\"x\":\"\xff\"}}")))),
	}
	var cases []opCase
	for _, name := range sortedKeys(mcInputs) {
		in := mcInputs[name]
		cfg := decode(in)
		err := ignition.ReplaceMachineOSURL(cfg, "quay.io/new@sha256:abc&x=<y>")
		cases = append(cases, opCase{Name: name, Input: in, Arg: "quay.io/new@sha256:abc&x=<y>", Result: raw(cfg), Err: errStr(err)})
	}
	write("machineos.json", cases)

	// EnableMTU
	pf := "/etc/NetworkManager/system-connections/pf0vf0.nmconnection"
	mtuInputs := map[string]string{
		"empty":  mk(),
		"pf0vf0": mk(f(pf, s("data:text/plain,"+url.QueryEscape("[connection]\nid=pf0vf0\n\n[ethernet]\nwake-on-lan=0\n[ethernet]\n")))),
		"no_eth": mk(f(pf, s("data:text/plain,"+url.QueryEscape("[connection]\n")))),
		"suffix": mk(f("/sysroot"+pf, s("data:text/plain,"+url.QueryEscape("[ethernet]\n"))), f(pf+".bak", s("data:,%5Bethernet%5D%0A"))),
		"nil":    mk(f(pf, nil), f(pf, s(""))),
		"target": targetContentJSON(false),
	}
	cases = nil
	for _, name := range sortedKeys(mtuInputs) {
		for _, mtu := range []uint16{9000, 1500, 0, 65535} {
			cfg := decode(mtuInputs[name])
			ignition.EnableMTU(cfg, mtu)
			cases = append(cases, opCase{Name: name, Input: mtuInputs[name], Arg: fmt.Sprint(mtu), Result: raw(cfg)})
		}
	}
	write("mtu.json", cases)

	// AddFlavorOVSScript
	cases = nil
	for i, script := range []string{"", "#!/bin/bash\novs-vsctl add-br br0", "<&>\u2028ünï\n", strings.Repeat("x", 1000)} {
		cfg := ignition.NewEmptyIgnition("3.4.0")
		ignition.AddFlavorOVSScript(cfg, &dpuprovisioningv1alpha1.DPUFlavorSpec{OVS: dpuprovisioningv1alpha1.DPUFlavorOVS{RawConfigScript: script}})
		cases = append(cases, opCase{Name: fmt.Sprint(i), Arg: script, Result: raw(cfg)})
	}
	write("ovs.json", cases)

	// URLEncode + EncodeGzipFile
	cases = nil
	for _, in := range []string{"", "hello", "a b+c&d=e/f?g#h%i\n", "ünïcødé\u2028", "<>&'\"", strings.Repeat("abc", 5000), "\x00\xff\x7f"} {
		res, err := ignition.EncodeGzipFile([]byte(in))
		must(err)
		cases = append(cases, opCase{Name: "gzip", Arg: base64.StdEncoding.EncodeToString([]byte(in)), Result: *res.Source + "|" + *res.Compression})
		cases = append(cases, opCase{Name: "urlencode", Arg: base64.StdEncoding.EncodeToString([]byte(in)), Result: ignition.URLEncode(in)})
	}
	write("encode.json", cases)
}

// ------------------------------------------------------------------------------------------

type flavorCase struct {
	Name   string `json:"name"`
	Flavor string `json:"flavor"`
	YAML   string `json:"yaml"`
	JSON   string `json:"json"`
}

func flavorFixtures() {
	filled, err := os.ReadFile("crates/apis/tests/fixtures/DPUFlavor.filled.json")
	must(err)
	flavors := map[string]string{
		"filled":  string(filled),
		"minimal": `{"metadata":{"name":"my-flavor","namespace":"ns","resourceVersion":"999","uid":"u","labels":{"a":"b"}},"spec":{"ovs":{"rawConfigScript":"#!/bin/bash"}}}`,
		"typed":   `{"apiVersion":"provisioning.dpu.nvidia.com/v1alpha1","kind":"DPUFlavor","metadata":{"name":"f","namespace":"dpf-operator-system","annotations":{"x":"y"}},"spec":{"grub":{"kernelParameters":["console=hvc0","hugepages=<&>"]},"sysctl":{"parameters":["net.ipv4.ip_forward=1"]},"nvconfig":[{"device":"*","parameters":["PF_BAR2_ENABLE=0"]}],"ovs":{"rawConfigScript":"_ovs-vsctl() {\n  ovs-vsctl --no-wait \"$@\"\n}\n"},"bfcfgParameters":["UPDATE_ATF_UEFI=yes"],"configFiles":[{"path":"/etc/mellanox/mlnx-bf.conf","operation":"override","raw":"ALLOW_SHARED_RQ=\"no\"\n","permissions":"0644"}],"dpuResources":{"cpu":"16","memory":"16Gi"},"systemReservedResources":{"cpu":"500m","memory":"1.5Gi"},"unknownField":{"a":1}}}`,
		"empty":   `{}`,
	}
	var cases []flavorCase
	for _, name := range sortedKeys(flavors) {
		var fl dpuprovisioningv1alpha1.DPUFlavor
		must(json.Unmarshal([]byte(flavors[name]), &fl))
		y := ignition.NewEmptyIgnition("3.4.0")
		must(ignition.AddDPUFlavorYAML(y, &fl))
		j := ignition.NewEmptyIgnition("3.4.0")
		must(ignition.AddDPUFlavorJSON(j, &fl))
		cases = append(cases, flavorCase{Name: name, Flavor: flavors[name], YAML: raw(y), JSON: raw(j)})
	}
	write("flavor.json", cases)
}

// ------------------------------------------------------------------------------------------

func targetContentJSON(zt bool) string {
	cfg := ignition.NewEmptyIgnition("3.4.0")
	must(content.AddContent(cfg, target.NewProvider(zt)))
	return raw(cfg)
}

type contentCase struct {
	Name    string `json:"name"`
	Result  string `json:"result"`
	Marshal string `json:"marshal"`
	Encode  string `json:"encode"`
}

func contentFixtures() {
	var cases []contentCase
	add := func(name string, cfg *igntypes.Config) {
		m, err := ignition.MarshalJSON(cfg)
		must(err)
		e, err := ignition.EncodeIgnition(cfg)
		must(err)
		cases = append(cases, contentCase{Name: name, Result: raw(cfg), Marshal: string(m), Encode: e})
	}
	providers := map[string]func(bool) *content.EmbeddedProvider{
		"common": common.NewProvider, "live": live.NewProvider, "target": target.NewProvider,
	}
	for _, pname := range []string{"common", "live", "target"} {
		for _, zt := range []bool{false, true} {
			p := providers[pname](zt)
			cfg := ignition.NewEmptyIgnition("3.4.0")
			must(content.AddContent(cfg, p))
			add(fmt.Sprintf("%s_zt_%v", pname, zt), cfg)
			cfg = ignition.NewEmptyIgnition("")
			must(content.AddFiles(cfg, p))
			add(fmt.Sprintf("%s_files_zt_%v", pname, zt), cfg)
			cfg = ignition.NewEmptyIgnition("3.2.0")
			must(content.AddLinks(cfg, p))
			add(fmt.Sprintf("%s_links_zt_%v", pname, zt), cfg)
			cfg = ignition.NewEmptyIgnition("3.4.0")
			must(content.AddSystemdUnits(cfg, p))
			add(fmt.Sprintf("%s_units_zt_%v", pname, zt), cfg)
		}
	}
	cfg := ignition.NewEmptyIgnition("3.4.0")
	content.AddKernelArgs(cfg)
	content.AddKernelArgs(cfg)
	add("kernel_args", cfg)

	// the full ignitiongenerator-like composition
	for _, zt := range []bool{false, true} {
		t := ignition.NewEmptyIgnition("3.4.0")
		must(content.AddContent(t, target.NewProvider(zt)))
		name, unit, err := target.RenderDPUAgentServiceUnit(zt)
		must(err)
		t.Systemd.Units = append(t.Systemd.Units, igntypes.Unit{Name: name, Enabled: ignition.Ptr(true), Contents: &unit})
		must(content.AddContent(t, common.NewProvider(zt)))
		add(fmt.Sprintf("target_full_zt_%v", zt), t)
		c := t
		must(ignition.GzipIgnitionFiles(c))
		add(fmt.Sprintf("target_full_gzip_zt_%v", zt), c)
	}
	write("content.json", cases)

	type unitCase struct {
		ZeroTrust bool   `json:"zero_trust"`
		Name      string `json:"name"`
		Contents  string `json:"contents"`
	}
	var units []unitCase
	for _, zt := range []bool{false, true} {
		n, c, err := target.RenderDPUAgentServiceUnit(zt)
		must(err)
		units = append(units, unitCase{zt, n, c})
	}
	write("dpu_agent_unit.json", units)
}

func main() {
	if len(os.Args) > 1 {
		outDir = os.Args[1]
	}
	must(os.MkdirAll(outDir, 0o755))
	configFixtures()
	decodeFixtures()
	mergeFixtures()
	opFixtures()
	flavorFixtures()
	contentFixtures()
}
