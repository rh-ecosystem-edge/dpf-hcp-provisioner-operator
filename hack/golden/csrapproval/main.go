// Command csrapproval generates golden fixtures for the Rust port of
// internal/controller/csrapproval (crates/ctrl-csrapproval): a corpus of CSR request bytes
// (real CSRs created with crypto/x509 for every supported key type, plus hand-built DER and
// PEM variants exercising encoding/pem, encoding/asn1 and crypto/x509 corner cases) together
// with what the Go code returns for them:
//
//   - x509.ParseCertificateRequest of the first PEM block (error or CN/O/SANs),
//   - csrapproval.ExtractHostname for a bootstrap CSR carrying the request,
//   - csrapproval.ValidateBootstrapCSR / ValidateServingCSR with valid identities.
//
// Usage: go run -mod=vendor ./hack/golden/csrapproval crates/ctrl-csrapproval/tests/fixtures
package main

import (
	"crypto"
	"crypto/ecdsa"
	"crypto/ed25519"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/rsa"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/asn1"
	"encoding/json"
	"encoding/pem"
	"fmt"
	"net"
	"net/url"
	"os"
	"path/filepath"

	certv1 "k8s.io/api/certificates/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"

	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/csrapproval"
)

const hostname = "dpu-worker-01"

// Case is one corpus entry.
type Case struct {
	Name    string `json:"name"`
	Request []byte `json:"request"`

	PEMFound bool   `json:"pemFound"`
	PEMType  string `json:"pemType"`

	ParseError string   `json:"parseError"`
	CN         string   `json:"cn"`
	Org        []string `json:"org"`
	DNS        []string `json:"dns"`
	Emails     []string `json:"emails"`
	IPs        []string `json:"ips"`
	URIs       []string `json:"uris"`

	ExtractHostname string `json:"extractHostname"`
	ExtractError    string `json:"extractError"`
	BootstrapError  string `json:"bootstrapError"`
	ServingError    string `json:"servingError"`
}

func must[T any](v T, err error) T {
	if err != nil {
		panic(err)
	}
	return v
}

func pemCSR(der []byte) []byte {
	return pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE REQUEST", Bytes: der})
}

// --- real CSRs ----------------------------------------------------------------------------

func realCSR(key crypto.Signer, tmpl x509.CertificateRequest) []byte {
	return must(x509.CreateCertificateRequest(rand.Reader, &tmpl, key))
}

func nodeSubject() pkix.Name {
	return pkix.Name{CommonName: "system:node:" + hostname, Organization: []string{"system:nodes"}}
}

// --- hand-built DER -----------------------------------------------------------------------

func marshal(v any) []byte { return must(asn1.Marshal(v)) }

// tlv builds a DER element with the given identifier octet and contents.
func tlv(tag byte, contents []byte) []byte {
	var l []byte
	n := len(contents)
	switch {
	case n < 0x80:
		l = []byte{byte(n)}
	case n < 0x100:
		l = []byte{0x81, byte(n)}
	default:
		l = []byte{0x82, byte(n >> 8), byte(n)}
	}
	out := append([]byte{tag}, l...)
	return append(out, contents...)
}

func seq(parts ...[]byte) []byte {
	var c []byte
	for _, p := range parts {
		c = append(c, p...)
	}
	return tlv(0x30, c)
}

func set(parts ...[]byte) []byte {
	var c []byte
	for _, p := range parts {
		c = append(c, p...)
	}
	return tlv(0x31, c)
}

func oid(o asn1.ObjectIdentifier) []byte { return marshal(o) }

var (
	oidCN            = asn1.ObjectIdentifier{2, 5, 4, 3}
	oidO             = asn1.ObjectIdentifier{2, 5, 4, 10}
	oidExtRequest    = asn1.ObjectIdentifier{1, 2, 840, 113549, 1, 9, 14}
	oidSAN           = asn1.ObjectIdentifier{2, 5, 29, 17}
	oidKeyUsage      = asn1.ObjectIdentifier{2, 5, 29, 15}
	oidRSA           = asn1.ObjectIdentifier{1, 2, 840, 113549, 1, 1, 1}
	oidEC            = asn1.ObjectIdentifier{1, 2, 840, 10045, 2, 1}
	oidEd25519       = asn1.ObjectIdentifier{1, 3, 101, 112}
	oidDSA           = asn1.ObjectIdentifier{1, 2, 840, 10040, 4, 1}
	oidP256          = asn1.ObjectIdentifier{1, 2, 840, 10045, 3, 1, 7}
	oidSecp256k1     = asn1.ObjectIdentifier{1, 3, 132, 0, 10}
	oidChallengePass = asn1.ObjectIdentifier{1, 2, 840, 113549, 1, 9, 7}
	oidUnknownAlg    = asn1.ObjectIdentifier{1, 2, 3, 4}
	oidECDSAWithSHA2 = asn1.ObjectIdentifier{1, 2, 840, 10045, 4, 3, 2}
)

// atv builds an AttributeTypeAndValue with a raw value element.
func atv(o asn1.ObjectIdentifier, value []byte) []byte { return seq(oid(o), value) }

func printable(s string) []byte { return tlv(0x13, []byte(s)) }
func utf8s(s string) []byte     { return tlv(0x0c, []byte(s)) }

func defaultSubject() []byte {
	return seq(set(atv(oidO, printable("system:nodes"))), set(atv(oidCN, utf8s("system:node:"+hostname))))
}

var ecKey = must(ecdsa.GenerateKey(elliptic.P256(), rand.Reader))

func defaultSPKI() []byte { return must(x509.MarshalPKIXPublicKey(&ecKey.PublicKey)) }

func extRequestAttr(exts ...[]byte) []byte {
	return seq(oid(oidExtRequest), set(seq(exts...)))
}

func ext(o asn1.ObjectIdentifier, value []byte) []byte {
	return seq(oid(o), tlv(0x04, value))
}

func sanExt(names ...[]byte) []byte { return ext(oidSAN, seq(names...)) }

func dnsName(s string) []byte { return tlv(0x82, []byte(s)) }
func ipName(b []byte) []byte  { return tlv(0x87, b) }
func uriName(s string) []byte { return tlv(0x86, []byte(s)) }
func email(s string) []byte   { return tlv(0x81, []byte(s)) }

type parts struct {
	version  []byte
	subject  []byte
	spki     []byte
	attrs    []byte // full [0] element, nil = omit
	sigAlg   []byte
	sig      []byte
	trailing []byte
}

func defaultParts(attrs ...[]byte) parts {
	return parts{
		version: marshal(0),
		subject: defaultSubject(),
		spki:    defaultSPKI(),
		attrs:   tlv(0xa0, concat(attrs...)),
		sigAlg:  seq(oid(oidECDSAWithSHA2)),
		sig:     tlv(0x03, []byte{0x00, 0x01, 0x02}),
	}
}

func concat(ps ...[]byte) []byte {
	var out []byte
	for _, p := range ps {
		out = append(out, p...)
	}
	return out
}

func (p parts) der() []byte {
	tbs := seq(p.version, p.subject, p.spki, p.attrs)
	return append(seq(tbs, p.sigAlg, p.sig), p.trailing...)
}

// --- evaluation ---------------------------------------------------------------------------

func strs(v []string) []string {
	if v == nil {
		return []string{}
	}
	return v
}

func evaluate(name string, request []byte) Case {
	c := Case{Name: name, Request: request, Org: []string{}, DNS: []string{}, Emails: []string{}, IPs: []string{}, URIs: []string{}}
	block, _ := pem.Decode(request)
	if block != nil {
		c.PEMFound = true
		c.PEMType = block.Type
		cr, err := x509.ParseCertificateRequest(block.Bytes)
		if err != nil {
			c.ParseError = err.Error()
		} else {
			c.CN = cr.Subject.CommonName
			c.Org = strs(cr.Subject.Organization)
			c.DNS = strs(cr.DNSNames)
			c.Emails = strs(cr.EmailAddresses)
			for _, ip := range cr.IPAddresses {
				c.IPs = append(c.IPs, ip.String())
			}
			for _, u := range cr.URIs {
				c.URIs = append(c.URIs, u.String())
			}
		}
	}

	bootstrap := &certv1.CertificateSigningRequest{
		ObjectMeta: metav1.ObjectMeta{Name: name},
		Spec: certv1.CertificateSigningRequestSpec{
			SignerName: csrapproval.SignerBootstrap,
			Username:   csrapproval.BootstrapperUsername,
			Groups: []string{
				"system:serviceaccounts",
				"system:serviceaccounts:openshift-machine-config-operator",
				"system:authenticated",
			},
			Usages:  []certv1.KeyUsage{certv1.UsageDigitalSignature, certv1.UsageClientAuth},
			Request: request,
		},
	}
	if h, err := csrapproval.ExtractHostname(bootstrap); err != nil {
		c.ExtractError = err.Error()
	} else {
		c.ExtractHostname = h
	}
	if err := csrapproval.ValidateBootstrapCSR(bootstrap, hostname); err != nil {
		c.BootstrapError = err.Error()
	}

	serving := &certv1.CertificateSigningRequest{
		ObjectMeta: metav1.ObjectMeta{Name: name},
		Spec: certv1.CertificateSigningRequestSpec{
			SignerName: csrapproval.SignerServing,
			Username:   "system:node:" + hostname,
			Groups:     []string{"system:nodes", "system:authenticated"},
			Usages:     []certv1.KeyUsage{certv1.UsageDigitalSignature, certv1.UsageServerAuth},
			Request:    request,
		},
	}
	if err := csrapproval.ValidateServingCSR(serving, hostname); err != nil {
		c.ServingError = err.Error()
	}
	return c
}

func main() {
	if len(os.Args) != 2 {
		fmt.Fprintln(os.Stderr, "usage: csrapproval <fixtures-dir>")
		os.Exit(2)
	}
	var cases []Case
	add := func(name string, request []byte) { cases = append(cases, evaluate(name, request)) }
	addDER := func(name string, der []byte) { add(name, pemCSR(der)) }

	rsaKey := must(rsa.GenerateKey(rand.Reader, 2048))
	p224 := must(ecdsa.GenerateKey(elliptic.P224(), rand.Reader))
	p384 := must(ecdsa.GenerateKey(elliptic.P384(), rand.Reader))
	p521 := must(ecdsa.GenerateKey(elliptic.P521(), rand.Reader))
	_, edKey := must2(ed25519.GenerateKey(rand.Reader))

	// Real CSRs for every key type.
	bootstrapTmpl := x509.CertificateRequest{Subject: nodeSubject()}
	servingTmpl := x509.CertificateRequest{Subject: nodeSubject(), DNSNames: []string{hostname}, IPAddresses: []net.IP{net.ParseIP("10.0.0.5")}}
	addDER("rsa-bootstrap", realCSR(rsaKey, bootstrapTmpl))
	addDER("rsa-serving", realCSR(rsaKey, servingTmpl))
	addDER("p224-serving", realCSR(p224, servingTmpl))
	addDER("p256-bootstrap", realCSR(ecKey, bootstrapTmpl))
	addDER("p256-serving", realCSR(ecKey, servingTmpl))
	addDER("p384-serving", realCSR(p384, servingTmpl))
	addDER("p521-serving", realCSR(p521, servingTmpl))
	addDER("ed25519-serving", realCSR(edKey, servingTmpl))

	// Subject variants (real CSRs).
	addDER("cn-only", realCSR(ecKey, x509.CertificateRequest{Subject: pkix.Name{CommonName: "system:node:" + hostname}}))
	addDER("cn-invalid-format", realCSR(ecKey, x509.CertificateRequest{Subject: pkix.Name{CommonName: "invalid-format"}}))
	addDER("cn-empty-hostname", realCSR(ecKey, x509.CertificateRequest{Subject: pkix.Name{CommonName: "system:node:"}}))
	addDER("cn-other-host", realCSR(ecKey, x509.CertificateRequest{Subject: pkix.Name{CommonName: "system:node:other", Organization: []string{"system:nodes"}}}))
	addDER("org-two", realCSR(ecKey, x509.CertificateRequest{Subject: pkix.Name{CommonName: "system:node:" + hostname, Organization: []string{"system:nodes", "x"}}}))
	addDER("org-wrong", realCSR(ecKey, x509.CertificateRequest{Subject: pkix.Name{CommonName: "system:node:" + hostname, Organization: []string{"wrong-org"}}}))

	// SAN variants (real CSRs).
	san := func(name string, tmpl x509.CertificateRequest) {
		tmpl.Subject = nodeSubject()
		addDER(name, realCSR(ecKey, tmpl))
	}
	san("san-two-dns", x509.CertificateRequest{DNSNames: []string{hostname, "extra"}})
	san("san-wrong-dns", x509.CertificateRequest{DNSNames: []string{"wrong-hostname"}})
	san("san-no-dns-ip-only", x509.CertificateRequest{IPAddresses: []net.IP{net.ParseIP("10.0.0.5")}})
	for _, ip := range []string{"127.0.0.1", "0.0.0.0", "::", "::1", "224.0.0.1", "ff02::1", "fe80::1", "2001:db8::1", "::ffff:10.1.2.3"} {
		san("san-ip-"+ip, x509.CertificateRequest{DNSNames: []string{hostname}, IPAddresses: []net.IP{net.ParseIP(ip)}})
	}
	san("san-email", x509.CertificateRequest{DNSNames: []string{hostname}, EmailAddresses: []string{"a@b.c"}})
	san("san-uri", x509.CertificateRequest{DNSNames: []string{hostname}, URIs: []*url.URL{must(url.Parse("spiffe://cluster.local/ns/x"))}})
	san("san-ip-v4-and-v6", x509.CertificateRequest{IPAddresses: []net.IP{net.ParseIP("10.0.0.5"), net.ParseIP("2001:db8::5").To16()}})

	// Hand-built DER: structure and encoding corner cases.
	addDER("der-default", defaultParts().der())
	addDER("der-no-attributes-element", func() []byte { p := defaultParts(); p.attrs = nil; return p.der() }())
	addDER("der-trailing-data", func() []byte { p := defaultParts(); p.trailing = []byte{0x00}; return p.der() }())
	addDER("der-truncated", func() []byte { d := defaultParts().der(); return d[:len(d)-1] }())
	addDER("der-empty", []byte{})
	addDER("der-version-nonminimal", func() []byte { p := defaultParts(); p.version = []byte{0x02, 0x02, 0x00, 0x00}; return p.der() }())
	addDER("der-version-too-large", func() []byte {
		p := defaultParts()
		p.version = tlv(0x02, []byte{1, 2, 3, 4, 5, 6, 7, 8, 9})
		return p.der()
	}())
	addDER("der-version-wrong-tag", func() []byte { p := defaultParts(); p.version = tlv(0x04, []byte{0}); return p.der() }())
	addDER("der-indefinite-length", []byte{0x30, 0x80, 0x00, 0x00})
	addDER("der-nonminimal-length", func() []byte {
		d := defaultParts().der()
		// re-encode outer length with a superfluous byte
		inner := d[2:]
		if d[1]&0x80 != 0 {
			inner = d[2+int(d[1]&0x7f):]
		}
		return append([]byte{0x30, 0x83, 0x00, byte(len(inner) >> 8), byte(len(inner))}, inner...)
	}())
	addDER("der-bad-signature-bits", func() []byte { p := defaultParts(); p.sig = tlv(0x03, []byte{0x08, 0x00}); return p.der() }())
	addDER("der-empty-signature-bits", func() []byte { p := defaultParts(); p.sig = tlv(0x03, nil); return p.der() }())
	addDER("der-extra-tbs-field", func() []byte {
		p := defaultParts()
		tbs := seq(p.version, p.subject, p.spki, p.attrs, marshal(5))
		return seq(tbs, p.sigAlg, p.sig)
	}())
	addDER("der-attrs-wrong-tag", func() []byte { p := defaultParts(); p.attrs = tlv(0xa1, nil); return p.der() }())
	addDER("der-attrs-primitive", func() []byte { p := defaultParts(); p.attrs = tlv(0x80, nil); return p.der() }())

	// Subject DER variants.
	subj := func(name string, subject []byte) {
		p := defaultParts()
		p.subject = subject
		addDER(name, p.der())
	}
	cn := func(v []byte) []byte { return set(atv(oidCN, v)) }
	o := set(atv(oidO, printable("system:nodes")))
	subj("subj-printable", seq(o, cn(printable("system:node:"+hostname))))
	subj("subj-printable-invalid-char", seq(o, cn(printable("system:node:dpu_worker"))))
	subj("subj-printable-asterisk-ampersand", seq(o, cn(printable("system:node:a*&b"))))
	subj("subj-ia5", seq(o, cn(tlv(0x16, []byte("system:node:"+hostname)))))
	subj("subj-ia5-nonascii", seq(o, cn(tlv(0x16, []byte("system:node:\xe9")))))
	subj("subj-t61-latin1", seq(o, cn(tlv(0x14, []byte("system:node:caf\xe9")))))
	subj("subj-utf8-invalid", seq(o, cn(tlv(0x0c, []byte("system:node:\xff")))))
	subj("subj-numeric-bad", seq(o, cn(tlv(0x12, []byte("12a")))))
	subj("subj-bmp", seq(o, cn(tlv(0x1e, utf16be("system:node:"+hostname)))))
	subj("subj-bmp-odd", seq(o, cn(tlv(0x1e, []byte{0x00, 0x61, 0x00}))))
	subj("subj-bmp-surrogate", seq(o, cn(tlv(0x1e, []byte{0xd8, 0x00}))))
	subj("subj-universalstring-ignored", seq(o, cn(tlv(0x1c, []byte{0, 0, 0, 0x61}))))
	subj("subj-integer-value-ignored", seq(o, cn(marshal(42))))
	subj("subj-integer-too-large", seq(o, cn(tlv(0x02, []byte{1, 2, 3, 4, 5, 6, 7, 8, 9}))))
	subj("subj-bool-invalid", seq(o, cn(tlv(0x01, []byte{0x01}))))
	subj("subj-constructed-value-ignored", seq(o, cn(seq(printable("x")))))
	subj("subj-context-value-ignored", seq(o, cn(tlv(0x80, []byte("x")))))
	subj("subj-utctime-valid", seq(o, cn(utf8s("system:node:"+hostname)), set(atv(asn1.ObjectIdentifier{2, 5, 4, 99}, tlv(0x17, []byte("240101120000Z"))))))
	subj("subj-utctime-invalid", seq(o, cn(utf8s("system:node:"+hostname)), set(atv(asn1.ObjectIdentifier{2, 5, 4, 99}, tlv(0x17, []byte("241301120000Z"))))))
	subj("subj-gentime-valid", seq(o, cn(utf8s("system:node:"+hostname)), set(atv(asn1.ObjectIdentifier{2, 5, 4, 99}, tlv(0x18, []byte("20240101120000.5Z"))))))
	subj("subj-gentime-trailing-zero", seq(o, cn(utf8s("system:node:"+hostname)), set(atv(asn1.ObjectIdentifier{2, 5, 4, 99}, tlv(0x18, []byte("20240101120000.50Z"))))))
	subj("subj-two-cn-last-wins", seq(o, cn(utf8s("system:node:first")), cn(utf8s("system:node:"+hostname))))
	subj("subj-multi-valued-rdn", seq(set(atv(oidO, printable("system:nodes")), atv(oidCN, utf8s("system:node:"+hostname)))))
	subj("subj-empty", seq())
	subj("subj-empty-rdn", seq(set(), o, cn(utf8s("system:node:"+hostname))))
	subj("subj-rdn-not-set", seq(seq(atv(oidCN, utf8s("system:node:"+hostname)))))
	subj("subj-not-sequence", set(o))
	subj("subj-atv-missing-value", seq(set(seq(oid(oidCN)))))
	subj("subj-atv-extra-field", seq(o, set(seq(oid(oidCN), utf8s("system:node:"+hostname), utf8s("x")))))
	subj("subj-o-twice", seq(o, set(atv(oidO, printable("system:nodes"))), cn(utf8s("system:node:"+hostname))))

	// Public key variants.
	spki := func(name string, key []byte) {
		p := defaultParts()
		p.spki = key
		addDER(name, p.der())
	}
	ecPoint := elliptic.Marshal(elliptic.P256(), ecKey.PublicKey.X, ecKey.PublicKey.Y)
	bitString := func(b []byte) []byte { return tlv(0x03, append([]byte{0}, b...)) }
	spki("key-unknown-algorithm", seq(seq(oid(oidUnknownAlg)), bitString([]byte{1, 2, 3})))
	spki("key-ec-unsupported-curve", seq(seq(oid(oidEC), oid(oidSecp256k1)), bitString(ecPoint)))
	spki("key-ec-missing-params", seq(seq(oid(oidEC)), bitString(ecPoint)))
	offCurve := append([]byte{}, ecPoint...)
	offCurve[len(offCurve)-1] ^= 1
	spki("key-ec-off-curve", seq(seq(oid(oidEC), oid(oidP256)), bitString(offCurve)))
	spki("key-ec-compressed", seq(seq(oid(oidEC), oid(oidP256)), bitString(elliptic.MarshalCompressed(elliptic.P256(), ecKey.PublicKey.X, ecKey.PublicKey.Y))))
	spki("key-ec-short", seq(seq(oid(oidEC), oid(oidP256)), bitString(ecPoint[:40])))
	bigX := append([]byte{4}, bytesRepeat(0xff, 64)...)
	spki("key-ec-coordinate-too-large", seq(seq(oid(oidEC), oid(oidP256)), bitString(bigX)))
	rsaPub := marshal(struct{ N, E any }{rsaKey.N, rsaKey.E})
	spki("key-rsa-no-null-params", seq(seq(oid(oidRSA)), bitString(rsaPub)))
	spki("key-rsa-negative-modulus", seq(seq(oid(oidRSA), []byte{0x05, 0x00}), bitString(seq(marshal(-5), marshal(65537)))))
	spki("key-rsa-zero-exponent", seq(seq(oid(oidRSA), []byte{0x05, 0x00}), bitString(seq(marshal(rsaKey.N), marshal(0)))))
	spki("key-rsa-huge-exponent", seq(seq(oid(oidRSA), []byte{0x05, 0x00}), bitString(seq(marshal(rsaKey.N), tlv(0x02, []byte{1, 2, 3, 4, 5, 6, 7, 8, 9})))))
	spki("key-rsa-garbage", seq(seq(oid(oidRSA), []byte{0x05, 0x00}), bitString([]byte{1, 2, 3})))
	spki("key-ed25519-wrong-size", seq(seq(oid(oidEd25519)), bitString([]byte{1, 2, 3})))
	spki("key-ed25519-params", seq(seq(oid(oidEd25519), []byte{0x05, 0x00}), bitString(make([]byte, 32))))
	spki("key-dsa-valid", seq(seq(oid(oidDSA), seq(marshal(23), marshal(11), marshal(4))), bitString(marshal(8))))
	spki("key-dsa-zero-param", seq(seq(oid(oidDSA), seq(marshal(23), marshal(0), marshal(4))), bitString(marshal(8))))
	spki("key-dsa-bad-params", seq(seq(oid(oidDSA), marshal(1)), bitString(marshal(8))))
	spki("key-spki-not-sequence", tlv(0x04, []byte{1}))
	spki("key-bitstring-padding", seq(seq(oid(oidUnknownAlg)), tlv(0x03, []byte{0x01, 0x01})))

	// Attribute / extension variants.
	attrs := func(name string, as ...[]byte) { addDER(name, defaultParts(as...).der()) }
	attrs("ext-san-dns", extRequestAttr(sanExt(dnsName(hostname))))
	attrs("ext-san-dns-nonascii", extRequestAttr(sanExt(dnsName("h\xe9"))))
	attrs("ext-san-email-nonascii", extRequestAttr(sanExt(email("\xe9@x"))))
	attrs("ext-san-ip-bad-length", extRequestAttr(sanExt(dnsName(hostname), ipName([]byte{1, 2, 3, 4, 5}))))
	attrs("ext-san-uri-invalid", extRequestAttr(sanExt(dnsName(hostname), uriName("http://a b"))))
	attrs("ext-san-uri-bad-port", extRequestAttr(sanExt(dnsName(hostname), uriName("http://x:abc/"))))
	attrs("ext-san-uri-bad-escape", extRequestAttr(sanExt(dnsName(hostname), uriName("/a%zz"))))
	attrs("ext-san-uri-colon-first-segment", extRequestAttr(sanExt(dnsName(hostname), uriName("a:b/c"))))
	attrs("ext-san-uri-missing-scheme", extRequestAttr(sanExt(dnsName(hostname), uriName(":x"))))
	attrs("ext-san-uri-invalid-domain", extRequestAttr(sanExt(dnsName(hostname), uriName("https://a..b/"))))
	attrs("ext-san-uri-trailing-dot", extRequestAttr(sanExt(dnsName(hostname), uriName("https://a.b./"))))
	attrs("ext-san-uri-ipv6", extRequestAttr(sanExt(dnsName(hostname), uriName("https://[::1]:443/x"))))
	attrs("ext-san-uri-ipv4-literal", extRequestAttr(sanExt(dnsName(hostname), uriName("https://[1.2.3.4]/x"))))
	attrs("ext-san-uri-opaque", extRequestAttr(sanExt(dnsName(hostname), uriName("urn:x:y"))))
	attrs("ext-san-uri-userinfo", extRequestAttr(sanExt(dnsName(hostname), uriName("https://u:p@h/x"))))
	attrs("ext-san-uri-bad-userinfo", extRequestAttr(sanExt(dnsName(hostname), uriName("https://u^p@h/x"))))
	attrs("ext-san-uri-fragment-bad-escape", extRequestAttr(sanExt(dnsName(hostname), uriName("https://h/x#%zz"))))
	attrs("ext-san-othername-ignored", extRequestAttr(sanExt(dnsName(hostname), tlv(0xa0, seq(oid(oidCN), tlv(0xa0, utf8s("x")))))))
	attrs("ext-san-registeredid-ignored", extRequestAttr(sanExt(dnsName(hostname), tlv(0x88, []byte{0x2a}))))
	attrs("ext-san-not-sequence", extRequestAttr(ext(oidSAN, set(dnsName(hostname)))))
	attrs("ext-san-truncated-name", extRequestAttr(ext(oidSAN, seq(dnsName(hostname))[:5])))
	attrs("ext-san-high-tag", extRequestAttr(ext(oidSAN, seq([]byte{0x9f, 0x21, 0x00}))))
	attrs("ext-san-trailing-after-seq", extRequestAttr(ext(oidSAN, append(seq(dnsName(hostname)), 0x00))))
	attrs("ext-duplicate", extRequestAttr(sanExt(dnsName(hostname)), sanExt(dnsName(hostname))))
	attrs("ext-duplicate-across-attrs", extRequestAttr(sanExt(dnsName(hostname))), extRequestAttr(sanExt(dnsName(hostname))))
	attrs("ext-critical", extRequestAttr(seq(oid(oidSAN), tlv(0x01, []byte{0xff}), tlv(0x04, seq(dnsName(hostname))))))
	attrs("ext-critical-invalid-bool", extRequestAttr(seq(oid(oidSAN), tlv(0x01, []byte{0x01}), tlv(0x04, seq(dnsName(hostname))))))
	attrs("ext-key-usage-ignored", extRequestAttr(ext(oidKeyUsage, []byte{0x03, 0x02, 0x05, 0xa0}), sanExt(dnsName(hostname))))
	attrs("ext-request-not-sequence", seq(oid(oidExtRequest), set(set())))
	attrs("ext-request-empty-set-ignored", seq(oid(oidExtRequest), set()))
	attrs("ext-request-two-values", seq(oid(oidExtRequest), set(seq(sanExt(dnsName(hostname))), seq(sanExt(dnsName("other"))))))
	attrs("attr-unparseable-ignored", tlv(0x04, []byte{1}), extRequestAttr(sanExt(dnsName(hostname))))
	attrs("attr-challenge-password", seq(oid(oidChallengePass), set(printable("secret"))), extRequestAttr(sanExt(dnsName(hostname))))
	attrs("attr-trailing-in-attr-ignored", seq(oid(oidExtRequest), set(seq(sanExt(dnsName("x")))), marshal(1)), extRequestAttr(sanExt(dnsName(hostname))))
	attrs("ext-request-extension-missing-value", extRequestAttr(seq(oid(oidSAN))))

	// PEM variants.
	valid := pemCSR(realCSR(ecKey, servingTmpl))
	add("pem-none", []byte("not a pem"))
	add("pem-empty", []byte{})
	add("pem-junk-before-after", concat([]byte("junk\n"), valid, []byte("trailer")))
	add("pem-crlf", crlf(valid))
	add("pem-wrong-type", pem.EncodeToMemory(&pem.Block{Type: "NEW CERTIFICATE REQUEST", Bytes: realCSR(ecKey, servingTmpl)}))
	add("pem-with-headers", pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE REQUEST", Headers: map[string]string{"Proc-Type": "4,ENCRYPTED"}, Bytes: realCSR(ecKey, servingTmpl)}))
	add("pem-bad-base64", []byte("-----BEGIN CERTIFICATE REQUEST-----\n!!!!\n-----END CERTIFICATE REQUEST-----\n"))
	add("pem-bad-then-good", concat([]byte("-----BEGIN CERTIFICATE REQUEST-----\n!!!!\n-----END CERTIFICATE REQUEST-----\n"), valid))
	add("pem-mismatched-end", []byte("-----BEGIN CERTIFICATE REQUEST-----\nAQID\n-----END CERTIFICATE-----\n"))
	add("pem-empty-body", []byte("-----BEGIN CERTIFICATE REQUEST-----\n-----END CERTIFICATE REQUEST-----\n"))
	add("pem-spaces-in-body", spacey(valid))
	add("pem-two-blocks-first-wins", concat(pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE REQUEST", Bytes: realCSR(ecKey, bootstrapTmpl)}), valid))
	add("pem-missing-padding", []byte("-----BEGIN CERTIFICATE REQUEST-----\nAQI\n-----END CERTIFICATE REQUEST-----\n"))
	add("pem-end-line-trailing-text", []byte("-----BEGIN CERTIFICATE REQUEST-----\nAQID\n-----END CERTIFICATE REQUEST----- x\n"))

	out := must(json.MarshalIndent(cases, "", "  "))
	path := filepath.Join(os.Args[1], "csr_corpus.json")
	if err := os.MkdirAll(os.Args[1], 0o755); err != nil {
		panic(err)
	}
	if err := os.WriteFile(path, append(out, '\n'), 0o644); err != nil {
		panic(err)
	}
	fmt.Printf("wrote %d cases to %s\n", len(cases), path)
}

func must2[A, B any](a A, b B, err error) (A, B) {
	if err != nil {
		panic(err)
	}
	return a, b
}

func bytesRepeat(b byte, n int) []byte {
	out := make([]byte, n)
	for i := range out {
		out[i] = b
	}
	return out
}

func utf16be(s string) []byte {
	var out []byte
	for _, r := range s {
		out = append(out, byte(r>>8), byte(r))
	}
	return out
}

func crlf(b []byte) []byte {
	var out []byte
	for _, c := range b {
		if c == '\n' {
			out = append(out, '\r')
		}
		out = append(out, c)
	}
	return out
}

// spacey inserts spaces and tabs inside base64 lines.
func spacey(b []byte) []byte {
	var out []byte
	inBody := false
	for i, c := range b {
		out = append(out, c)
		if c == '\n' {
			inBody = i+1 < len(b) && b[i+1] != '-'
			if inBody {
				out = append(out, ' ', '\t')
			}
		}
	}
	return out
}
