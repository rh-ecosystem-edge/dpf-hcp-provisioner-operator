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

// Command gocompat generates the golden differential-test fixtures of crates/gocompat:
// inputs (fixed-seed random corpora plus hand-picked edge cases) are run through the Go
// implementations and the exact outputs / error messages are written to
// crates/gocompat/tests/fixtures/*.json.
//
// Run from the repository root: go run -mod=vendor ./hack/golden/gocompat
package main

import (
	"bytes"
	"compress/gzip"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"math"
	"math/rand"
	"net/url"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"unicode/utf8"

	blang "github.com/blang/semver/v4"
	coreos "github.com/coreos/go-semver/semver"
	"sigs.k8s.io/yaml"
)

const fixtureDir = "crates/gocompat/tests/fixtures"

// write stores v as compact JSON with one top-level array element per line.
func write(name string, v any) {
	var b []byte
	var err error
	emitList := func(list []any) []byte {
		out := []byte("[\n")
		for i, e := range list {
			eb, err := json.Marshal(e)
			if err != nil {
				panic(err)
			}
			out = append(out, eb...)
			if i < len(list)-1 {
				out = append(out, ',')
			}
			out = append(out, '\n')
		}
		return append(out, ']')
	}
	// round-trip through any to iterate generically
	raw, err := json.Marshal(v)
	if err != nil {
		panic(err)
	}
	var generic any
	dec := json.NewDecoder(bytes.NewReader(raw))
	dec.UseNumber()
	if err := dec.Decode(&generic); err != nil {
		panic(err)
	}
	switch g := generic.(type) {
	case []any:
		b = emitList(g)
	case map[string]any:
		b = []byte("{\n")
		keys := make([]string, 0, len(g))
		for k := range g {
			keys = append(keys, k)
		}
		sort.Strings(keys)
		for i, k := range keys {
			kb, _ := json.Marshal(k)
			b = append(b, kb...)
			b = append(b, ':')
			if l, ok := g[k].([]any); ok {
				b = append(b, emitList(l)...)
			} else {
				eb, _ := json.Marshal(g[k])
				b = append(b, eb...)
			}
			if i < len(keys)-1 {
				b = append(b, ',')
			}
			b = append(b, '\n')
		}
		b = append(b, '}')
	default:
		b = raw
	}
	b = append(b, '\n')
	if err := os.WriteFile(filepath.Join(fixtureDir, name), b, 0o644); err != nil {
		panic(err)
	}
	fmt.Printf("wrote %s (%d bytes)\n", name, len(b))
}

func pick(r *rand.Rand, pool string) byte { return pool[r.Intn(len(pool))] }

func hx(b []byte) string { return hex.EncodeToString(b) }

func errStr(err error) *string {
	if err == nil {
		return nil
	}
	s := err.Error()
	return &s
}

// ---------------------------------------------------------------------------------------------
// typed value encoding shared with the Rust tests:
//   ["n"] ["b",bool] ["i","<int64>"] ["u","<uint64>"] ["f","<float64 bits hex>"]
//   ["s","<hex>"] ["a",[...]] ["o",[["<key hex>",v],...]]
// ---------------------------------------------------------------------------------------------

func enc(v any) any {
	switch x := v.(type) {
	case nil:
		return []any{"n"}
	case bool:
		return []any{"b", x}
	case int64:
		return []any{"i", strconv.FormatInt(x, 10)}
	case uint64:
		return []any{"u", strconv.FormatUint(x, 10)}
	case float64:
		return []any{"f", strconv.FormatUint(math.Float64bits(x), 16)}
	case string:
		return []any{"s", hx([]byte(x))}
	case []any:
		out := make([]any, 0, len(x))
		for _, e := range x {
			out = append(out, enc(e))
		}
		return []any{"a", out}
	case map[string]any:
		keys := make([]string, 0, len(x))
		for k := range x {
			keys = append(keys, k)
		}
		sort.Strings(keys)
		out := make([]any, 0, len(x))
		for _, k := range keys {
			out = append(out, []any{hx([]byte(k)), enc(x[k])})
		}
		return []any{"o", out}
	}
	panic(fmt.Sprintf("enc: %T", v))
}

// ---------------------------------------------------------------------------------------------
// random generators
// ---------------------------------------------------------------------------------------------

var specialStrings = []string{
	"", " ", "  ", "y", "Y", "yes", "Yes", "YES", "n", "N", "no", "on", "On", "off", "OFF", "true", "True",
	"false", "FALSE", "null", "Null", "NULL", "~", "<<", ".nan", ".NaN", ".inf", "-.inf", "+.inf", ".Inf",
	"0", "1", "-1", "+1", "007", "0x1F", "0X1f", "0o17", "0b101", "-0b101", "0b2", "1_000", "1__0", "_1",
	"1e3", "1E3", "1e+3", "1.", ".5", "-.5", "+.5", "1.5e-3", "1.5e", "1e", "e3", "._5", ".5_5", ".5e1_0",
	"12345678901234567890", "-9223372036854775808", "9223372036854775807", "9223372036854775808",
	"18446744073709551615", "18446744073709551616", "1e400", "-1e400", ".1e400",
	"1:20", "1:20:30", "-1:20", "+1:20.5", "1:2:3.4_5", "1:60", "1:5", "190:20:30.15", "1:", ":1", "1::2",
	"2001-12-14", "2001-12-14t21:59:43.10-05:00", "2001-12-14T21:59:43.10Z", "2001-12-14 21:59:43.10",
	"2001-12-14  21:59:43", "2002-12-14 1:2:3", "2001-2-29", "2000-2-29", "2001-13-01", "2001-12-32",
	"2001-12-14T25:00:00Z", "2001-12-14T21:59:43+24:00", "2001-12-14T21:59:43+25:00", "2001-12-14T1:2:3,5Z",
	"2001-12-14T21:59:43", "2001-12-14x", "20011-12-14", "2001-12-1", "2001-1-1 1:1:1.", "2001-1-1 ",
	"-", "--", "---", "--- a", "...", "....", "- a", "-a", "? a", "?a", "?", ":", ": a", "a:", "a: b", "a:b",
	"a #b", "a#b", "#a", "a,b", "[a]", "{a}", "a]", "a}", "&a", "*a", "!a", "|", ">", "'", "\"", "%a", "@a",
	"`a", "a'b", "a\"b", "a\\b", "\\", "a b", " a", "a ", "a  b", "\ta", "a\t", "a\tb",
	"\n", "a\n", "\na", "a\nb", "a\n\nb", "a\n \nb", "a \nb", "a\n b", " a\nb", "a\nb ", "a\nb\n", "a\nb\n\n",
	"\n\n", "a\r\nb", "a\rb", "a\u0085b", "a\u2028b", "a\u2029b", "\u00a0", "a\u00a0", "\ufeff", "\ufeffa",
	"a\ufeff", "\x00", "a\x01b", "\x1b", "\x7f", "a\x7fb", "\u0080", "\u0084", "\u0085", "\u009f", "\ufffe",
	"\uffff", "\ufffd", "é", "中文", "😀", "a😀b", "<tag>", "a&b", "a>b",
	"=", "+", ".", "..", "0.", "-0", "+0", "-0.0", "0e0", "0x", "0b", "0o", "0xG", "0_x1", "1.2.3", "1,000",
	"Inf", "inf", "NaN", "nan", "infinity", "-Infinity", "+inf",
}

const alphabet = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"

var runePool = []rune{
	' ', ' ', ' ', '\n', '\t', '\r', ':', '#', '-', '?', ',', '[', ']', '{', '}', '&', '*', '!', '|', '>',
	'\'', '"', '%', '@', '`', '\\', '/', '<', '>', '&', '.', '_', '~', '=', '+', 0, 1, 0x1b, 0x7f, 0x80, 0x85,
	0x9f, 0xa0, 0xe9, 0x2028, 0x2029, 0xfeff, 0xfffd, 0x4e2d, 0x1f600, 0x10ffff, 0xd7ff, 0xe000, 0x3000,
}

func randString(r *rand.Rand, allowBad bool) string {
	switch r.Intn(10) {
	case 0, 1:
		s := specialStrings[r.Intn(len(specialStrings))]
		if !allowBad && hasBad(s) {
			return "x"
		}
		return s
	case 2:
		// long words for line folding
		n := 5 + r.Intn(60)
		var sb strings.Builder
		for i := 0; i < n; i++ {
			if i > 0 {
				sb.WriteString(strings.Repeat(" ", 1+r.Intn(2)*r.Intn(3)))
			}
			w := 1 + r.Intn(12)
			for j := 0; j < w; j++ {
				sb.WriteByte(alphabet[r.Intn(len(alphabet))])
			}
		}
		return sb.String()
	case 3:
		// multi-line text
		n := 1 + r.Intn(6)
		var sb strings.Builder
		if r.Intn(4) == 0 {
			sb.WriteString(strings.Repeat(" ", r.Intn(3)))
		}
		for i := 0; i < n; i++ {
			w := r.Intn(30)
			for j := 0; j < w; j++ {
				if r.Intn(6) == 0 {
					sb.WriteByte(' ')
				} else {
					sb.WriteByte(alphabet[r.Intn(len(alphabet))])
				}
			}
			sb.WriteString(strings.Repeat("\n", 1+r.Intn(2)*r.Intn(3)))
		}
		s := sb.String()
		if r.Intn(2) == 0 {
			s = strings.TrimRight(s, "\n")
		}
		return s
	default:
		n := r.Intn(25)
		var sb strings.Builder
		for i := 0; i < n; i++ {
			if r.Intn(3) == 0 {
				c := runePool[r.Intn(len(runePool))]
				if !allowBad && isBadRune(c) {
					c = 'q'
				}
				sb.WriteRune(c)
			} else {
				sb.WriteByte(alphabet[r.Intn(len(alphabet))])
			}
		}
		return sb.String()
	}
}

func isBadRune(c rune) bool {
	return c == 0x7f || (c >= 0x80 && c <= 0x9f && c != 0x85) || c == 0xfffe || c == 0xffff
}

func hasBad(s string) bool {
	for _, c := range s {
		if isBadRune(c) {
			return true
		}
	}
	return false
}

func randFloat(r *rand.Rand) float64 {
	switch r.Intn(10) {
	case 0:
		return math.Float64frombits(r.Uint64() & 0x7fefffffffffffff) // finite random bits
	case 1:
		return -math.Float64frombits(r.Uint64() & 0x7fefffffffffffff)
	case 2:
		return float64(r.Int63n(2000000) - 1000000)
	case 3:
		return math.Copysign(0, -1)
	case 4:
		return r.Float64() * math.Pow(10, float64(r.Intn(60)-30))
	case 5:
		return math.Pow(10, float64(r.Intn(50)-25)) * float64(r.Intn(3)-1)
	case 6:
		vals := []float64{1e21, 1e20, 9.999999999999999e20, 1e-6, 1e-7, 9.99999e-7, 0.1, 0.2, 0.3, 1.0 / 3,
			123456789, 1234567, 123456, 100000, 1e6, 1e15, 1e16, 1e17, 4.940656458412465e-324,
			1.7976931348623157e308, 2.2250738585072014e-308, 0.000001, 0.0000001, 1e100, 5e-324,
			math.MaxInt64, math.MaxUint64, 9007199254740993, -1e21, 12345678901234567890}
		return vals[r.Intn(len(vals))]
	default:
		return float64(r.Intn(1000)) / float64(1+r.Intn(16))
	}
}

func randValue(r *rand.Rand, depth int, allowBad bool) any {
	k := r.Intn(12)
	if depth > 4 {
		k = 4 + r.Intn(8)
	}
	switch k {
	case 0, 1:
		n := r.Intn(6)
		m := make(map[string]any, n)
		for i := 0; i < n; i++ {
			m[randString(r, allowBad)] = randValue(r, depth+1, allowBad)
		}
		return m
	case 2, 3:
		n := r.Intn(5)
		a := make([]any, 0, n)
		for i := 0; i < n; i++ {
			a = append(a, randValue(r, depth+1, allowBad))
		}
		return a
	case 4:
		return nil
	case 5:
		return r.Intn(2) == 0
	case 6:
		return randFloat(r)
	case 7:
		switch r.Intn(4) {
		case 0:
			return int64(r.Uint64())
		case 1:
			return int64(r.Intn(100000) - 50000)
		default:
			return int64(r.Intn(10))
		}
	case 8:
		if r.Intn(3) == 0 {
			return uint64(math.MaxInt64) + 1 + r.Uint64()%(math.MaxUint64-math.MaxInt64)
		}
		return randFloat(r)
	default:
		return randString(r, allowBad)
	}
}

// ---------------------------------------------------------------------------------------------
// strconv
// ---------------------------------------------------------------------------------------------

func genStrconv() {
	r := rand.New(rand.NewSource(1))
	type quoteCase struct {
		In  string `json:"in"`
		Out string `json:"out"`
	}
	var quotes []quoteCase
	addQ := func(b []byte) { quotes = append(quotes, quoteCase{hx(b), strconv.Quote(string(b))}) }
	for i := 0; i < 256; i++ {
		addQ([]byte{byte(i)})
	}
	for c := rune(0); c < 0x800; c++ {
		addQ([]byte(string(c)))
	}
	for i := 0; i < 5000; i++ {
		c := rune(r.Intn(0x110000))
		addQ([]byte(string(c)))
	}
	for i := 0; i < 1500; i++ {
		n := r.Intn(12)
		b := make([]byte, n)
		for j := range b {
			b[j] = byte(r.Intn(256))
		}
		addQ(b)
		addQ([]byte(randString(r, true)))
	}

	type intCase struct {
		Fn   string  `json:"fn"`
		S    string  `json:"s"`
		Base int     `json:"base"`
		Bits int     `json:"bits"`
		Val  string  `json:"val"`
		Err  *string `json:"err"`
	}
	var ints []intCase
	intInputs := []string{"", "0", "-0", "+0", "1", "-1", "+", "-", "9", "8", "7", "777", "0777", "0644", "644", "755",
		"0o17", "0O17", "0x1f", "0X1F", "0b101", "0B101", "0x", "0b", "0o", "1_000", "1__000", "_1000", "1000_",
		"0x_1f", "0_17", "0x1_f", "9223372036854775807", "9223372036854775808", "-9223372036854775808",
		"-9223372036854775809", "18446744073709551615", "18446744073709551616", "99999999999999999999",
		"2147483647", "2147483648", "-2147483648", "-2147483649", "4294967295", "4294967296", "zz", "ZZ", "z",
		"10", "１", "12a", " 1", "1 ", "0.5", "1e3", "abc", "-abc", "0xffffffffffffffff", "0x10000000000000000",
		"777777777777777777777", "1777777777777777777777", "2000000000000000000000", "17777777777"}
	for _, s := range intInputs {
		for _, base := range []int{0, 2, 8, 10, 16, 36, 1, 37} {
			for _, bits := range []int{0, 8, 32, 64, 65} {
				v, err := strconv.ParseInt(s, base, bits)
				ints = append(ints, intCase{"ParseInt", hx([]byte(s)), base, bits, strconv.FormatInt(v, 10), errStr(err)})
				u, err := strconv.ParseUint(s, base, bits)
				ints = append(ints, intCase{"ParseUint", hx([]byte(s)), base, bits, strconv.FormatUint(u, 10), errStr(err)})
			}
		}
		v, err := strconv.Atoi(s)
		ints = append(ints, intCase{"Atoi", hx([]byte(s)), 10, 0, strconv.Itoa(v), errStr(err)})
	}
	for i := 0; i < 1500; i++ {
		var s string
		switch r.Intn(3) {
		case 0:
			s = strconv.FormatInt(r.Int63()>>uint(r.Intn(63)), 8)
		case 1:
			b := make([]byte, r.Intn(8))
			for j := range b {
				b[j] = pick(r, "0123456789abcdefxo_+-")
			}
			s = string(b)
		default:
			s = strconv.FormatUint(r.Uint64()>>uint(r.Intn(64)), 10)
		}
		base := []int{0, 8, 10, 16}[r.Intn(4)]
		bits := []int{0, 32, 64}[r.Intn(3)]
		v, err := strconv.ParseInt(s, base, bits)
		ints = append(ints, intCase{"ParseInt", hx([]byte(s)), base, bits, strconv.FormatInt(v, 10), errStr(err)})
		u, err := strconv.ParseUint(s, base, bits)
		ints = append(ints, intCase{"ParseUint", hx([]byte(s)), base, bits, strconv.FormatUint(u, 10), errStr(err)})
		a, err := strconv.Atoi(s)
		ints = append(ints, intCase{"Atoi", hx([]byte(s)), 10, 0, strconv.Itoa(a), errStr(err)})
	}

	type floatFmtCase struct {
		Bits    string `json:"bits"`
		Fmt     string `json:"fmt"`
		Prec    int    `json:"prec"`
		BitSize int    `json:"bitsize"`
		Out     string `json:"out"`
	}
	var ffmt []floatFmtCase
	addF := func(f float64) {
		for _, fm := range []byte{'e', 'E', 'f', 'g', 'G'} {
			for _, bs := range []int{64, 32} {
				ffmt = append(ffmt, floatFmtCase{strconv.FormatUint(math.Float64bits(f), 16), string(fm), -1, bs,
					strconv.FormatFloat(f, fm, -1, bs)})
			}
		}
		for _, prec := range []int{0, 1, 3, 6, 17} {
			for _, fm := range []byte{'e', 'f', 'g'} {
				ffmt = append(ffmt, floatFmtCase{strconv.FormatUint(math.Float64bits(f), 16), string(fm), prec, 64,
					strconv.FormatFloat(f, fm, prec, 64)})
			}
		}
	}
	for _, f := range []float64{0, math.Copysign(0, -1), 1, -1, 0.1, 1e21, 1e20, 1e-6, 1e-7, math.Inf(1), math.Inf(-1),
		math.NaN(), math.MaxFloat64, math.SmallestNonzeroFloat64, math.MaxFloat32, 123456789, 1234567, 123456,
		0.000123, 1e100, 1.5, 2.5, 0.5, 0.25, 9.5, 100, 1e6} {
		addF(f)
	}
	for i := 0; i < 300; i++ {
		addF(randFloat(r))
	}
	for i := 0; i < 2000; i++ {
		f := math.Float64frombits(r.Uint64())
		ffmt = append(ffmt, floatFmtCase{strconv.FormatUint(math.Float64bits(f), 16), "g", -1, 64, strconv.FormatFloat(f, 'g', -1, 64)})
		ffmt = append(ffmt, floatFmtCase{strconv.FormatUint(math.Float64bits(f), 16), "e", -1, 64, strconv.FormatFloat(f, 'e', -1, 64)})
		f32 := float64(math.Float32frombits(r.Uint32()))
		ffmt = append(ffmt, floatFmtCase{strconv.FormatUint(math.Float64bits(f32), 16), "g", -1, 32, strconv.FormatFloat(f32, 'g', -1, 32)})
	}

	type parseFloatCase struct {
		S       string  `json:"s"`
		BitSize int     `json:"bitsize"`
		Bits    string  `json:"bits"`
		Err     *string `json:"err"`
	}
	var pf []parseFloatCase
	pfInputs := []string{"", "0", "-0", "1", "1.", ".5", ".", "-.5", "+.5", "1e5", "1E5", "1e+5", "1e-5", "1e", "e5",
		"1.5e309", "1e400", "-1e400", "1e-400", "inf", "+inf", "-Inf", "infinity", "INFINITY", "infin", "nan",
		"NaN", "+nan", "-nan", "0x1p-2", "0x1.8p1", "0x1p", "0x1", "0X1P+2", "0x.8p1", "0x1_0p0", "1_000.5",
		"1__0", "_1", "1_", "._5", ".5_5", "0.000001", "1e-7", "123456789012345678901234567890",
		"4.9406564584124654e-324", "2.4703282292062327e-324", "2.4703282292062328e-324", "1.7976931348623157e308",
		"1.7976931348623158e308", "1.7976931348623159e308", "3.4028235e38", "3.4028236e38", "1.401298464324817e-45",
		"0.1", "0.2", "0.30000000000000004", "1.5e", "1.5e+", "--1", "1..2", "1.2.3", " 1", "1 ", "0x", "0xg",
		"9007199254740993", "9007199254740993.0000000001", "1e10000", "1e-10000", ".5e1_0"}
	for _, s := range pfInputs {
		for _, bs := range []int{64, 32} {
			f, err := strconv.ParseFloat(s, bs)
			pf = append(pf, parseFloatCase{hx([]byte(s)), bs, strconv.FormatUint(math.Float64bits(f), 16), errStr(err)})
		}
	}
	for i := 0; i < 2000; i++ {
		f := randFloat(r)
		var s string
		switch r.Intn(3) {
		case 0:
			s = strconv.FormatFloat(f, 'g', -1, 64)
		case 1:
			s = strconv.FormatFloat(f, 'e', r.Intn(25), 64)
		default:
			s = strconv.FormatFloat(f, 'f', r.Intn(30), 64)
		}
		bs := []int{64, 32}[r.Intn(2)]
		v, err := strconv.ParseFloat(s, bs)
		pf = append(pf, parseFloatCase{hx([]byte(s)), bs, strconv.FormatUint(math.Float64bits(v), 16), errStr(err)})
	}

	write("strconv.json", map[string]any{"quote": quotes, "ints": ints, "format_float": ffmt, "parse_float": pf})
}

// ---------------------------------------------------------------------------------------------
// json + yaml
// ---------------------------------------------------------------------------------------------

type marshalCase struct {
	Input   any     `json:"input"`
	JSON    string  `json:"json"`
	Indent  string  `json:"indent"`
	YAML    *string `json:"yaml"`
	YAMLErr *string `json:"yaml_err"`
	// Result of sigs.k8s.io/yaml.Unmarshal(YAML, &any) re-marshalled with json.Marshal.
	Back    *string `json:"back"`
	BackErr *string `json:"back_err"`
}

func marshalCaseFor(v any) marshalCase {
	j, err := json.Marshal(v)
	if err != nil {
		panic(err)
	}
	ind, err := json.MarshalIndent(v, ">", "\t ")
	if err != nil {
		ind = nil
	}
	c := marshalCase{Input: enc(v), JSON: string(j), Indent: string(ind)}
	y, err := yaml.Marshal(v)
	if err != nil {
		c.YAMLErr = errStr(err)
	} else {
		s := string(y)
		c.YAML = &s
		var back any
		if err := yaml.Unmarshal(y, &back); err != nil {
			c.BackErr = errStr(err)
		} else {
			bj, err := json.Marshal(back)
			if err != nil {
				panic(err)
			}
			bs := string(bj)
			c.Back = &bs
		}
	}
	return c
}

func genJSONYAML() {
	r := rand.New(rand.NewSource(2))
	var cases []marshalCase
	// every special string as a value, as a key, in a sequence, and at the root
	for _, s := range specialStrings {
		cases = append(cases, marshalCaseFor(map[string]any{"k": s}))
		cases = append(cases, marshalCaseFor(map[string]any{s: "v"}))
		cases = append(cases, marshalCaseFor([]any{s, map[string]any{"a": s}}))
		cases = append(cases, marshalCaseFor(s))
		cases = append(cases, marshalCaseFor(map[string]any{"nested": map[string]any{"list": []any{s, []any{s}}}}))
	}
	for _, f := range []float64{0, math.Copysign(0, -1), 1, 1.5, 1e20, 1e21, 1e-6, 1e-7, 0.1, 123456789, 1e100,
		math.MaxFloat64, math.SmallestNonzeroFloat64, 9007199254740993, -1e21, 12345678901234567890} {
		cases = append(cases, marshalCaseFor(map[string]any{"f": f}))
	}
	for _, i := range []int64{0, -1, math.MaxInt64, math.MinInt64, 1 << 53} {
		cases = append(cases, marshalCaseFor(map[string]any{"i": i}))
	}
	cases = append(cases, marshalCaseFor(map[string]any{"u": uint64(math.MaxUint64)}))
	// key ordering (yaml.v2 natural sort)
	keySets := [][]string{
		{"a", "b", "A", "B", "_", "-", "1", "10", "2", "a1", "a10", "a2", "a01", "a001", "a0", "a00", "é", "z", "Z"},
		// note: yaml.v2's key comparison is not transitive for e.g. {"x10y", "x1y", "x02"}
		// (x10y < x1y < x01y < x2y < x02 < x10y), making Go's own output order random; such
		// sets are excluded.
		{"x1y", "x01y", "x2y", "x02", "x0", "item10", "item9", "item09", "item009", "item0010", "10a", "9a"},
		{"key", "key1", "key-1", "key_1", "key.1", "key 1", "key:1", "Key", "KEY", "kéy", "k中", "k😀", "k1٣", "k١"},
		{"apiVersion", "kind", "metadata", "spec", "status", "name", "namespace", "labels", "annotations"},
		{"0", "00", "000", "01", "001", "010", "0010", "1", "-1", "+1", "1.5", "1e3", "a0b", "a00b", "a0c"},
	}
	for _, ks := range keySets {
		m := map[string]any{}
		for i, k := range ks {
			m[k] = int64(i)
		}
		cases = append(cases, marshalCaseFor(m))
	}
	// long keys around the 128 (simple key) and 1024 (implicit key) limits
	for _, n := range []int{126, 127, 128, 129, 200, 1020, 1021, 1022, 1023, 1024} {
		cases = append(cases, marshalCaseFor(map[string]any{strings.Repeat("k", n): int64(1), "z": []any{int64(1)}}))
		cases = append(cases, marshalCaseFor(map[string]any{strings.Repeat("ab ", n/3): "v"}))
	}
	cases = append(cases, marshalCaseFor(map[string]any{strings.Repeat("<", 200): int64(1)}))
	cases = append(cases, marshalCaseFor(map[string]any{"multi\nline key": int64(1), "other": map[string]any{"x\ny": []any{int64(1)}}}))
	// long values (folding)
	for _, n := range []int{70, 79, 80, 81, 82, 100, 200, 500} {
		long := strings.TrimSpace(strings.Repeat("word ", n/5+1))[:n]
		cases = append(cases, marshalCaseFor(map[string]any{"v": long, "q": "'" + long, "d": "\x01" + long,
			"l": []any{long, map[string]any{"deep": map[string]any{"deeper": long}}}}))
		cases = append(cases, marshalCaseFor(map[string]any{"v": strings.Repeat("x", n) + " " + strings.Repeat("y", n)}))
		cases = append(cases, marshalCaseFor(map[string]any{"d": "\x01" + strings.Repeat("a  b ", n/5)}))
	}
	// depth limit (inputs rebuilt by the Rust test; only the outcome is recorded)
	type deepCase struct {
		Depth   int     `json:"depth"`
		Kind    string  `json:"kind"`
		YAMLLen int     `json:"yaml_len"`
		YAMLSHA string  `json:"yaml_sha256"`
		YAMLErr *string `json:"yaml_err"`
	}
	var deeps []deepCase
	for _, d := range []int{9999, 10000, 10001} {
		for _, kind := range []string{"array", "object"} {
			if kind == "object" && d < 10001 {
				// nested block mappings indent quadratically (100MB of output); only the
				// depth-limit error is checked for objects.
				continue
			}
			var deep any = int64(1)
			for i := 0; i < d; i++ {
				if kind == "array" {
					deep = []any{deep}
				} else {
					deep = map[string]any{"a": deep}
				}
			}
			c := deepCase{Depth: d, Kind: kind}
			y, err := yaml.Marshal(deep)
			if err != nil {
				c.YAMLErr = errStr(err)
			} else {
				sum := sha256.Sum256(y)
				c.YAMLLen = len(y)
				c.YAMLSHA = hx(sum[:])
			}
			deeps = append(deeps, c)
		}
	}
	write("yaml_deep.json", deeps)
	// random documents
	for i := 0; i < 2500; i++ {
		cases = append(cases, marshalCaseFor(randValue(r, 0, i%5 == 0)))
	}
	// a large realistic document
	big := map[string]any{}
	for i := 0; i < 200; i++ {
		big[fmt.Sprintf("key%d", i)] = randValue(r, 1, false)
	}
	cases = append(cases, marshalCaseFor(big))
	write("json_yaml_marshal.json", cases)

	// appendString over arbitrary bytes (via json.Marshal of a string containing them) and
	// HTMLEscape / Compact / Indent / Valid over arbitrary texts.
	type strCase struct {
		In  string `json:"in"`
		Out string `json:"out"`
	}
	var strs []strCase
	for i := 0; i < 256; i++ {
		b, _ := json.Marshal(string([]byte{byte(i)}))
		strs = append(strs, strCase{hx([]byte{byte(i)}), string(b)})
	}
	for i := 0; i < 3000; i++ {
		n := r.Intn(16)
		b := make([]byte, n)
		for j := range b {
			if r.Intn(3) == 0 {
				b[j] = byte(r.Intn(256))
			} else {
				const pool = "abc<>&\"\\\n\t\xe2\x80\xa8\xa9\xc3\xa9"
				b[j] = pool[r.Intn(len(pool))]
			}
		}
		o, _ := json.Marshal(string(b))
		strs = append(strs, strCase{hx(b), string(o)})
	}

	type textCase struct {
		In      string  `json:"in"`
		Valid   bool    `json:"valid"`
		SynErr  *string `json:"syn_err"`
		SynOff  int64   `json:"syn_off"`
		Compact *string `json:"compact"`
		Indent  *string `json:"indent"`
		HTML    string  `json:"html"`
		// json.Unmarshal into any
		Any    any     `json:"any"`
		AnyErr *string `json:"any_err"`
		// json.Unmarshal into map[string]any
		Map    any     `json:"map"`
		MapNil bool    `json:"map_nil"`
		MapErr *string `json:"map_err"`
	}
	var texts []textCase
	addText := func(b []byte) {
		c := textCase{In: hx(b), Valid: json.Valid(b)}
		light := len(b) > 15000
		var sink any
		if err := json.Unmarshal(b, &sink); err != nil {
			c.AnyErr = errStr(err)
			if se, ok := err.(*json.SyntaxError); ok {
				c.SynErr = errStr(se)
				c.SynOff = se.Offset
			}
		} else if !light {
			c.Any = enc(sink)
		}
		var m map[string]any
		if err := json.Unmarshal(b, &m); err != nil {
			c.MapErr = errStr(err)
		} else if m == nil {
			c.MapNil = true
		} else if !light {
			c.Map = enc(m)
		}
		if light {
			texts = append(texts, c)
			return
		}
		var cb bytes.Buffer
		if err := json.Compact(&cb, b); err == nil {
			s := hx(cb.Bytes())
			c.Compact = &s
		}
		var ib bytes.Buffer
		if err := json.Indent(&ib, b, "", "  "); err == nil {
			s := hx(ib.Bytes())
			c.Indent = &s
		}
		var hb bytes.Buffer
		json.HTMLEscape(&hb, b)
		c.HTML = hx(hb.Bytes())
		texts = append(texts, c)
	}
	handTexts := []string{"", " ", "null", "true", "false", "nul", "tru", "fals", "nulll", "0", "-0", "-", "01", "1.", "1.e5",
		"1e", "1e+", "1e+5", "-1.5E-3", "1e400", "-1e400", "1e-400", "123456789012345678901234567890", "\"\"", "\"",
		"\"\\\"", "\"\\u\"", "\"\\u12\"", "\"\\u12345\"", "\"\\ud83d\\ude00\"", "\"\\ud83d\"", "\"\\ude00\"",
		"\"\\ud83dx\"", "\"\\ud83d\\u0041\"", "\"\\x\"", "\"\\'\"", "\"\\/\"", "\"a\tb\"", "\"\x01\"", "\"\xff\"",
		"\"\xc3\"", "\"\xed\xa0\x80\"", "\"<>&\u2028\u2029\"", "[]", "[", "]", "[1,]", "[,1]", "[1 2]", "[1,2]",
		"{}", "{", "}", "{\"a\"}", "{\"a\":}", "{\"a\":1,}", "{\"a\":1 \"b\":2}", "{a:1}", "{\"a\":1}", "{\"a\":1}x",
		"{\"a\":1} ", " {\"a\" : 1 } ", "{\"a\":1,\"a\":2}", "{\"a\":{\"b\":[1,{\"c\":null}]}}", "[1e400]",
		"{\"a\":1e400}", "1 2", "\x00", "\ufeff{}", "{\"a\":\"\\u003c\"}", "[\"\\u2028\"]", "[true,false,null]",
		"[1,\"a\",{}]", "{\"\":\"\"}", "\"\\b\\f\\n\\r\\t\"", "[-]", "[-a]", "[0x1]", "[.5]", "[+1]", "[1.5e3.2]",
		"[\"a\"\"b\"]", "{\"a\" 1}", "{1:1}", "{\"a\":1,,}", "\t\r\n []\t\r\n"}
	for _, t := range handTexts {
		addText([]byte(t))
	}
	deepArr := strings.Repeat("[", 10001) + strings.Repeat("]", 10001)
	addText([]byte(deepArr))
	okArr := strings.Repeat("[", 10000) + strings.Repeat("]", 10000)
	addText([]byte(okArr))
	for i := 0; i < 2500; i++ {
		v := randValue(r, 0, true)
		var b []byte
		if r.Intn(2) == 0 {
			b, _ = json.Marshal(v)
		} else {
			b, _ = json.MarshalIndent(v, "", " ")
		}
		// mutate
		switch r.Intn(4) {
		case 0:
		case 1:
			if len(b) > 0 {
				b = b[:r.Intn(len(b))]
			}
		case 2:
			if len(b) > 0 {
				b[r.Intn(len(b))] = pick(r, "{}[],:\"\\ x0-e.\x00\xff")
			}
		case 3:
			pos := r.Intn(len(b) + 1)
			ins := []byte{pick(r, "{}[],:\"\\ x0-e.tfn\n")}
			b = append(b[:pos], append(ins, b[pos:]...)...)
		}
		addText(b)
	}
	write("json_texts.json", map[string]any{"strings": strs, "texts": texts})

	// JSONToYAML over JSON texts with exact number literals.
	type j2yCase struct {
		In   string  `json:"in"`
		Out  *string `json:"out"`
		Err  *string `json:"err"`
		YToJ *string `json:"y2j"`
	}
	var j2y []j2yCase
	numLits := []string{"0", "-0", "1", "-1", "1.0", "-0.0", "1.5", "1e3", "1E3", "1e+3", "1e-3", "1.5e300", "1e400",
		"-1e400", "1e-400", "12345678901234567890", "18446744073709551615", "18446744073709551616",
		"-9223372036854775808", "-9223372036854775809", "9223372036854775807", "9223372036854775808",
		"0.1", "100000000000000000000", "1e21", "1e20", "0.000001", "0.0000001", "123456789.123456789"}
	for _, n := range numLits {
		for _, t := range []string{"{\"n\":" + n + "}", "[" + n + "]", n, "{\"a\": [ " + n + " , {\"b\" : " + n + "} ] }"} {
			c := j2yCase{In: hx([]byte(t))}
			y, err := yaml.JSONToYAML([]byte(t))
			if err != nil {
				c.Err = errStr(err)
			} else {
				s := string(y)
				c.Out = &s
				back, err := yaml.YAMLToJSON(y)
				if err == nil {
					bs := string(back)
					c.YToJ = &bs
				}
			}
			j2y = append(j2y, c)
		}
	}
	for i := 0; i < 500; i++ {
		v := randValue(r, 0, false)
		b, _ := json.MarshalIndent(v, "", " ")
		c := j2yCase{In: hx(b)}
		y, err := yaml.JSONToYAML(b)
		if err != nil {
			c.Err = errStr(err)
		} else {
			s := string(y)
			c.Out = &s
			back, err := yaml.YAMLToJSON(y)
			if err == nil {
				bs := string(back)
				c.YToJ = &bs
			}
		}
		j2y = append(j2y, c)
	}
	write("json_to_yaml.json", j2y)

	// YAML documents (hand-written) through YAMLToJSON / Unmarshal(any).
	type y2jCase struct {
		In     string  `json:"in"`
		Out    *string `json:"out"`
		Err    *string `json:"err"`
		Any    *string `json:"any"`
		AnyErr *string `json:"any_err"`
	}
	var y2j []y2jCase
	docs := []string{
		"", "---\n", "a: 1\n", "a: 1", "a:\n  b: 2\n  c: [1, 2]\n", "- a\n- b\n- - c\n  - d\n", "a:\n- 1\n- 2\nb: 3\n",
		"a: |\n  line1\n  line2\n", "a: |-\n  x\n\n  y\n", "a: |+\n  x\n\n", "a: >\n  folded\n  text\n\n  para\n",
		"a: |2-\n    indented\n  b\n", "? |-\n  multi\n  key\n: v\n", "? " + strings.Repeat("k", 150) + "\n: v\n",
		"a: \"double \\\"q\\\" \\u00e9 \\x41 \\t\"\n", "a: 'single ''q'''\n", "a: \"fold\n  ed\"\n", "a: plain\n  continued\n",
		"a: 'x\n\n  y'\n", "a: \"x\\\n  y\"\n", "a: \"x\\ \n  y\"\n", "a: y\nb: yes\nc: on\nd: ~\ne: null\nf: 0x1F\ng: 1_000\nh: .5\n",
		"a: 2001-12-14\nb: 2001-12-14T21:59:43.10Z\n", "1: a\n2.5: b\ntrue: c\n", "a: .inf\n", "a: .nan\n",
		"a: 12345678901234567890\n", "a: 1e400\n", "a: -0\nb: -0.0\n", "# comment\na: 1 # trailing\n# end\n",
		"a: {b: 1, c: [x, \"y\"], d: {}}\n", "{\"a\": 1, \"b\": [true, null]}", "[1, 2, {a: b}]", "a: b: c\n",
		"a:\n  - b\n - c\n", "a: [1, 2\n", "a: 'unterminated\n", "- a\nb: 1\n", "a: 1\na: 2\n", "a: &x 1\nb: *x\n",
		"a: !!str 1\n", "a:\n\tb: 1\n", "key: \"\\N\\_\\L\\P\\e\\0\\a\\b\\v\\f\"\n", "a: \"\\U0001F600\"\n",
		"- |\n  a\n- >-\n  b\n  c\n", "a:\n  b:\n    c:\n      d: deep\n", "a: \"\"\nb: ''\n", "a:\nb:\n",
		"- \n- x\n", "a: x # c\nb: 'y' # c\n", "a: \"x\" \n", "a: |\n\n  x\n", "a: |\n  x\n \n  y\n",
		"---\na: 1\n---\nb: 2\n", "a: 1\n...\n", "a: [a: 1]\n", "a: http://example.com/x?y=1#z\n", "a: b:c\n",
		"a: -1\nb: - 1\n", "a:\n  -\n    b: 1\n", "a: x\n  # comment\n  y\n", "- - - x\n", "- a: 1\n  b: 2\n- c: 3\n",
	}
	for _, d := range docs {
		c := y2jCase{In: hx([]byte(d))}
		out, err := yaml.YAMLToJSON([]byte(d))
		if err != nil {
			c.Err = errStr(err)
		} else {
			s := string(out)
			c.Out = &s
		}
		var v any
		if err := yaml.Unmarshal([]byte(d), &v); err != nil {
			c.AnyErr = errStr(err)
		} else {
			b, _ := json.Marshal(v)
			s := string(b)
			c.Any = &s
		}
		y2j = append(y2j, c)
	}
	write("yaml_to_json.json", y2j)
}

// ---------------------------------------------------------------------------------------------
// gzip / flate
// ---------------------------------------------------------------------------------------------

// splitmix64, mirrored in the Rust tests.
type sm struct{ s uint64 }

func (r *sm) next() uint64 {
	r.s += 0x9E3779B97F4A7C15
	z := r.s
	z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9
	z = (z ^ (z >> 27)) * 0x94D049BB133111EB
	return z ^ (z >> 31)
}

var words = []string{"the", "quick", "brown", "fox", "jumps", "over", "lazy", "dog", "ignition", "systemd",
	"unit", "{", "}", "\"", ":", ",", "\n", "  ", "storage", "files", "contents", "source", "data", "base64",
	"version", "3.4.0", "0644", "/etc/", "kubernetes", "\t", "a", "b", "c"}

// payload must stay in sync with tests/common payload().
func payload(kind string, seed uint64, size int) []byte {
	r := &sm{seed}
	out := make([]byte, 0, size)
	switch kind {
	case "random":
		for len(out) < size {
			out = append(out, byte(r.next()))
		}
	case "zeros":
		out = make([]byte, size)
	case "text":
		for len(out) < size {
			out = append(out, words[r.next()%uint64(len(words))]...)
			out = append(out, ' ')
		}
	case "repeat":
		pat := make([]byte, 1+r.next()%61)
		for i := range pat {
			pat[i] = byte(r.next())
		}
		for len(out) < size {
			out = append(out, pat...)
		}
	case "lowentropy":
		for len(out) < size {
			out = append(out, "acgt"[r.next()%4])
		}
	case "mixed":
		for len(out) < size {
			n := int(1 + r.next()%5000)
			switch r.next() % 4 {
			case 0:
				for i := 0; i < n; i++ {
					out = append(out, byte(r.next()))
				}
			case 1:
				for i := 0; i < n; i++ {
					out = append(out, 0)
				}
			case 2:
				for len(out) < size && n > 0 {
					w := words[r.next()%uint64(len(words))]
					out = append(out, w...)
					n -= len(w)
				}
			default:
				// long-distance copies
				if len(out) > 0 {
					start := int(r.next() % uint64(len(out)))
					for i := 0; i < n && start+i < len(out); i++ {
						out = append(out, out[start+i])
					}
				}
			}
		}
	}
	return out[:size]
}

type gzOp struct {
	Op string `json:"op"` // "w" (write n bytes) or "f" (flush)
	N  int    `json:"n"`
}

type gzCase struct {
	Kind   string `json:"kind"`
	Seed   uint64 `json:"seed"`
	Size   int    `json:"size"`
	Level  int    `json:"level"`
	Ops    []gzOp `json:"ops"`
	Len    int    `json:"len"`
	SHA256 string `json:"sha256"`
	Hex    string `json:"hex,omitempty"`
}

func runGzip(data []byte, level int, ops []gzOp) []byte {
	var buf bytes.Buffer
	w, err := gzip.NewWriterLevel(&buf, level)
	if err != nil {
		panic(err)
	}
	if ops == nil {
		w.Write(data)
	} else {
		pos := 0
		for _, op := range ops {
			switch op.Op {
			case "w":
				end := pos + op.N
				if end > len(data) {
					end = len(data)
				}
				w.Write(data[pos:end])
				pos = end
			case "f":
				w.Flush()
			}
		}
		w.Write(data[pos:])
	}
	w.Close()
	return buf.Bytes()
}

func genGzip() {
	r := rand.New(rand.NewSource(3))
	var cases []gzCase
	add := func(kind string, seed uint64, size, level int, ops []gzOp) {
		data := payload(kind, seed, size)
		out := runGzip(data, level, ops)
		sum := sha256.Sum256(out)
		c := gzCase{Kind: kind, Seed: seed, Size: size, Level: level, Ops: ops, Len: len(out), SHA256: hx(sum[:])}
		if len(out) <= 1024 {
			c.Hex = hx(out)
		}
		cases = append(cases, c)
	}
	kinds := []string{"random", "zeros", "text", "repeat", "lowentropy", "mixed"}
	allLevels := []int{-2, -1, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9}
	smallSizes := []int{0, 1, 2, 3, 4, 5, 15, 16, 17, 31, 32, 100, 127, 128, 129, 257, 258, 259, 1000, 4095, 4096, 4097}
	seed := uint64(1)
	for _, k := range kinds {
		for _, sz := range smallSizes {
			for _, lv := range allLevels {
				add(k, seed, sz, lv, nil)
				seed++
			}
		}
	}
	midSizes := []int{16384, 32767, 32768, 32769, 65534, 65535, 65536, 65537, 100000}
	for _, k := range kinds {
		for _, sz := range midSizes {
			for _, lv := range allLevels {
				add(k, seed, sz, lv, nil)
				seed++
			}
		}
	}
	for _, k := range kinds {
		for _, lv := range []int{-2, -1, 0, 1, 4, 6, 9} {
			add(k, seed, 300000+int(seed%1000), lv, nil)
			seed++
		}
	}
	for _, k := range []string{"text", "mixed", "random", "repeat"} {
		for _, lv := range []int{-1, 1, 9} {
			add(k, seed, 1<<20, lv, nil)
			seed++
		}
	}
	// > 16MiB of input exercises the hash offset renormalisation (maxHashOffset)
	for _, lv := range []int{2, 6, 9} {
		add("mixed", seed, 17<<20+12345, lv, nil)
		seed++
	}
	add("random", seed, 17<<20, 4, nil)
	seed++
	// chunked writes and flushes
	for i := 0; i < 120; i++ {
		k := kinds[r.Intn(len(kinds))]
		lv := allLevels[r.Intn(len(allLevels))]
		size := r.Intn(200000)
		var ops []gzOp
		for n := 0; n < 1+r.Intn(30); n++ {
			if r.Intn(5) == 0 {
				ops = append(ops, gzOp{Op: "f"})
			} else {
				ops = append(ops, gzOp{Op: "w", N: []int{1, 7, 100, 4096, 30000, 70000}[r.Intn(6)]})
			}
		}
		add(k, seed, size, lv, ops)
		seed++
	}
	// invalid levels
	type badLevel struct {
		Level int    `json:"level"`
		Err   string `json:"err"`
	}
	var bad []badLevel
	for _, lv := range []int{-3, 10, 100} {
		_, err := gzip.NewWriterLevel(io.Discard, lv)
		bad = append(bad, badLevel{lv, err.Error()})
	}

	// decompression: valid streams (multistream too) and corrupted inputs
	type gunzipCase struct {
		In        string  `json:"in"`
		NewErr    *string `json:"new_err"`
		ReadErr   *string `json:"read_err"`
		OutSHA256 string  `json:"out_sha256"`
		OutLen    int     `json:"out_len"`
	}
	var gun []gunzipCase
	addGun := func(b []byte) {
		c := gunzipCase{In: hx(b)}
		zr, err := gzip.NewReader(bytes.NewReader(b))
		if err != nil {
			c.NewErr = errStr(err)
			gun = append(gun, c)
			return
		}
		var out bytes.Buffer
		_, err = out.ReadFrom(zr)
		if err != nil {
			c.ReadErr = errStr(err)
		} else {
			sum := sha256.Sum256(out.Bytes())
			c.OutSHA256 = hx(sum[:])
			c.OutLen = out.Len()
		}
		gun = append(gun, c)
	}
	for _, h := range []string{"", "1f", "1f8b", "1f8b08", "1f8b0800000000000000", "1f8b0700000000000000",
		"0000000000000000000000", "1f8b08080000000000ff6100", "1f8b08080000000000ff61", "1f8b08040000000000ff0200",
		"1f8b08040000000000ff020061", "1f8b08020000000000ff0000", "1f8b08100000000000ff63006300"} {
		b, _ := hex.DecodeString(h)
		addGun(b)
	}
	for i := 0; i < 300; i++ {
		k := kinds[r.Intn(len(kinds))]
		lv := allLevels[r.Intn(len(allLevels))]
		data := payload(k, uint64(5000+i), r.Intn(2500))
		z := runGzip(data, lv, nil)
		if r.Intn(4) == 0 {
			z = append(z, runGzip(payload(k, uint64(9000+i), r.Intn(1500)), lv, nil)...)
		}
		switch r.Intn(6) {
		case 0:
		case 1:
			z = z[:r.Intn(len(z))]
		case 2:
			p := r.Intn(len(z))
			z[p] ^= byte(1 << uint(r.Intn(8)))
		case 3:
			for j := 0; j < 1+r.Intn(5); j++ {
				p := 10 + r.Intn(len(z)-10)
				z[p] = byte(r.Intn(256))
			}
		case 4:
			z = append(z, byte(r.Intn(256)))
		case 5:
			z = append(z, 0x1f, 0x8b)
		}
		addGun(z)
	}
	write("gzip.json", map[string]any{"compress": cases, "bad_levels": bad, "gunzip": gun})
}

// ---------------------------------------------------------------------------------------------
// url, base64
// ---------------------------------------------------------------------------------------------

func genURLBase64() {
	r := rand.New(rand.NewSource(4))
	type escCase struct {
		In    string  `json:"in"`
		Query string  `json:"query"`
		Path  string  `json:"path"`
		QU    *string `json:"qu"`
		QUErr *string `json:"qu_err"`
		PU    *string `json:"pu"`
		PUErr *string `json:"pu_err"`
	}
	var cases []escCase
	add := func(s string) {
		c := escCase{In: hx([]byte(s)), Query: url.QueryEscape(s), Path: url.PathEscape(s)}
		if u, err := url.QueryUnescape(s); err != nil {
			c.QUErr = errStr(err)
		} else {
			h := hx([]byte(u))
			c.QU = &h
		}
		if u, err := url.PathUnescape(s); err != nil {
			c.PUErr = errStr(err)
		} else {
			h := hx([]byte(u))
			c.PU = &h
		}
		cases = append(cases, c)
	}
	for i := 0; i < 256; i++ {
		add(string([]byte{byte(i)}))
	}
	for _, s := range []string{"", "%", "%2", "%zz", "%2g", "%%", "%25", "a+b", "a b", "%20+%2B", "%e2%82%ac", "%E2%82%AC",
		"x%", "x%4", "%4x%", "100%", "[ethernet]\nmtu=9000\n", "data:,%7B%22a%22%3A1%7D", "%ff%fe", "%C3%A9+%"} {
		add(s)
	}
	for i := 0; i < 3000; i++ {
		n := r.Intn(30)
		b := make([]byte, n)
		for j := range b {
			if r.Intn(4) == 0 {
				b[j] = byte(r.Intn(256))
			} else {
				b[j] = pick(r, "%+ 0123456789abcdefABCDEFgz/?#&=;:@$,~._-")
			}
		}
		add(string(b))
		add(randString(r, true))
	}

	type b64Case struct {
		In  string  `json:"in"`
		Enc string  `json:"enc"`
		Dec *string `json:"dec"`
		Err *string `json:"err"`
	}
	var b64 []b64Case
	addB := func(b []byte) {
		c := b64Case{In: hx(b), Enc: base64.StdEncoding.EncodeToString(b)}
		if d, err := base64.StdEncoding.DecodeString(string(b)); err != nil {
			c.Err = errStr(err)
		} else {
			h := hx(d)
			c.Dec = &h
		}
		b64 = append(b64, c)
	}
	for _, s := range []string{"", "=", "==", "a", "ab", "abc", "abcd", "ab==", "abc=", "ab=", "a===", "abcd=", "ab==\n",
		"ab\n==", "ab=\n=", "a\nb\nc\nd", "\r\n", "abcd\r\nefgh", "ab=c", "ab==ab==", "YWJj", "YW Jj", "YW+/", "YW-_",
		"Zg==", "Zg", "Zm8=", "Zm8", "Zm9v", "Zh==", "Zm9=", "!!!!", "abcdefgh", "abcdefg=", "abcdef==", "abcde==="} {
		addB([]byte(s))
	}
	for i := 0; i < 3000; i++ {
		n := r.Intn(40)
		b := make([]byte, n)
		switch r.Intn(3) {
		case 0:
			for j := range b {
				b[j] = byte(r.Intn(256))
			}
		case 1:
			raw := make([]byte, r.Intn(30))
			r.Read(raw)
			b = []byte(base64.StdEncoding.EncodeToString(raw))
			if len(b) > 0 && r.Intn(2) == 0 {
				b[r.Intn(len(b))] = pick(r, "=\n\r!A/+")
			}
		default:
			for j := range b {
				b[j] = pick(r, "ABCabc019+/=\n\r")
			}
		}
		addB(b)
	}
	write("url_base64.json", map[string]any{"escape": cases, "base64": b64})
}

// ---------------------------------------------------------------------------------------------
// semver
// ---------------------------------------------------------------------------------------------

func genSemver() {
	inputs := []string{"", "1", "1.2", "1.2.3", "v1.2.3", "V1.2.3", " 1.2.3 ", "\t4.22.1\n", "01.2.3", "1.02.3", "1.2.03",
		"1.2.3-rc.1", "1.2.3-rc.01", "1.2.3-rc.1+build.5", "1.2.3+build", "1.2.3+", "1.2.3-", "1.2.3-a..b",
		"1.2.3-alpha", "1.2.3-alpha.1", "1.2.3-alpha.beta", "1.2.3-beta", "1.2.3-beta.2", "1.2.3-beta.11",
		"1.2.3-rc.1", "1.2.3-0", "1.2.3-00", "1.2.3-x-y", "1.2.3-x_y", "1.2.3+x_y", "1.2.3+a.b..c", "a.b.c",
		"1.2.c", "1.2.3.4", "1..3", ".1.2", "1.2.", "-1.2.3", "1.-2.3", "18446744073709551615.0.0",
		"18446744073709551616.0.0", "9223372036854775807.0.0", "9223372036854775808.0.0", "4.22", "4.22.0",
		"4.22.1", "4.22.2", "4.22.6", "4.21.10", "4.22.0-ec.3", "4.22.1-rc.1", "4.22", "v4.22", "4", "v4", "0.0.0",
		"3.4.0", "3.2.0", "3.0", "2.2.0", "3.4.0-experimental", "3.5.0-experimental", "3.x.0", "3.4", "3",
		"1.2-rc1", "1-rc1", "4.22.1-0.nightly-2025-01-01-000000", "1.2.3-é", "1.2.3+é", "00.0.0", "0.00.0",
		"1.2.3-1.2.3", "1.2.3-a.1", "1.2.3-1.a", "1.2.3-11", "1.2.3-2", "1.2.3-a", "1.2.3-b", "1.2.3-a.b.c"}
	type pcase struct {
		In         string  `json:"in"`
		CoreOS     *string `json:"coreos"`
		CoreOSErr  *string `json:"coreos_err"`
		CoreOSMaj  int64   `json:"coreos_major"`
		Blang      *string `json:"blang"`
		BlangErr   *string `json:"blang_err"`
		Tolerant   *string `json:"tolerant"`
		TolErr     *string `json:"tolerant_err"`
		BlangFinal string  `json:"blang_final"`
	}
	var parses []pcase
	for _, in := range inputs {
		c := pcase{In: in}
		if v, err := coreos.NewVersion(in); err != nil {
			c.CoreOSErr = errStr(err)
		} else {
			s := v.String()
			c.CoreOS = &s
			c.CoreOSMaj = v.Major
		}
		if v, err := blang.Parse(in); err != nil {
			c.BlangErr = errStr(err)
		} else {
			s := v.String()
			c.Blang = &s
			c.BlangFinal = v.FinalizeVersion()
		}
		if v, err := blang.ParseTolerant(in); err != nil {
			c.TolErr = errStr(err)
		} else {
			s := v.String()
			c.Tolerant = &s
		}
		parses = append(parses, c)
	}
	type ccase struct {
		A      string `json:"a"`
		B      string `json:"b"`
		CoreOS *int   `json:"coreos"`
		Blang  *int   `json:"blang"`
	}
	var cmps []ccase
	for _, a := range inputs {
		for _, b := range inputs {
			c := ccase{A: a, B: b}
			va, ea := coreos.NewVersion(a)
			vb, eb := coreos.NewVersion(b)
			if ea == nil && eb == nil {
				x := va.Compare(*vb)
				c.CoreOS = &x
			}
			ba, ea2 := blang.ParseTolerant(a)
			bb, eb2 := blang.ParseTolerant(b)
			if ea2 == nil && eb2 == nil {
				x := ba.Compare(bb)
				c.Blang = &x
			}
			if c.CoreOS != nil || c.Blang != nil {
				cmps = append(cmps, c)
			}
		}
	}
	write("semver.json", map[string]any{"parse": parses, "compare": cmps})
}

func main() {
	if !utf8.ValidString("é") {
		panic("source encoding")
	}
	if err := os.MkdirAll(fixtureDir, 0o755); err != nil {
		panic(err)
	}
	genStrconv()
	genJSONYAML()
	genGzip()
	genURLBase64()
	genSemver()
}
