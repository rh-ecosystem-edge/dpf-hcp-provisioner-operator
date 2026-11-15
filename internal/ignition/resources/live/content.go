package live

import (
	"embed"

	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/ignition/content"
)

//go:embed files/*
var filesFS embed.FS

//go:embed systemd/*
var systemdFS embed.FS

const nl = "%0A" // URL-encoded newline for data URIs

// NewProvider builds the live-ignition content provider. bootstrapKubeconfig
// controls whether the dpu-agent bootstrap kubeconfig template is embedded;
// bfbRegistryURLTemplate controls whether the BFBRegistryURL environment
// template parameter is included; caTrustBundleTemplate independently controls
// whether the CA template is included for supported DPF versions.
func NewProvider(zeroTrust, bootstrapKubeconfig, bfbRegistryURLTemplate, caTrustBundleTemplate bool) *content.EmbeddedProvider {
	f := func(name string) []byte { return content.EmbedFile(filesFS, "files/"+name) }

	dpuMode := "trusted"
	if zeroTrust {
		dpuMode = "zero-trust"
	}

	envSource := "data:," +
		"DPUName={{.DPUName}}" + nl +
		"DPUNamespace={{.DPUNamespace}}" + nl +
		"DPUUID={{.DPUUID}}" + nl +
		"DPUMode=" + dpuMode + nl +
		"ControlPlaneMTU={{.ControlPlaneMTU}}" + nl
	// DPF only populates BFBRegistryURL for Redfish installs, independently of security mode.
	if bfbRegistryURLTemplate {
		envSource += "BFBRegistryURL={{.BFBRegistryURL}}" + nl
	}

	files := []content.FileDefinition{
		{
			// Trick DPF checking for the existence of these strings in /etc/bf.env
			Path:          "/etc/temp_bfcfg_strings.env",
			Mode:          0644,
			ContentSource: "data:,bfb_pre_install%20bfb_modify_os%20bfb_post_install",
		},
		{
			Path:          "/etc/hostname",
			Mode:          0644,
			ContentSource: "data:,{{.DPUHostName}}",
		},
		{
			Path:          "/etc/dpf/environment",
			Mode:          0644,
			ContentSource: envSource,
		},
		{
			Path:          "/usr/local/bin/install-rhcos-dpf.sh",
			Mode:          0755,
			ContentSource: f("install-rhcos-dpf.sh"),
		},
		{
			Path:          "/usr/local/bin/update_ignition.py",
			Mode:          0755,
			ContentSource: f("update_ignition.py"),
		},
	}

	if caTrustBundleTemplate {
		files = append(files, content.FileDefinition{
			// DPF supplies raw PEM; encode it when the outer bf.cfg template is rendered.
			Path:          "/etc/pki/ca-trust/source/anchors/dpf-ca.crt",
			Mode:          0644,
			ContentSource: "data:;base64,{{ .CATrustBundle | b64enc }}",
		})
	}

	if bootstrapKubeconfig {
		files = append(files, content.FileDefinition{
			Path:          "/var/lib/dpf/dpuagent/bootstrap-kubeconfig",
			Mode:          0600,
			ContentSource: "data:;base64,{{ .BootstrapKubeconfig | b64enc }}",
		})
	}

	return &content.EmbeddedProvider{
		Files:     files,
		SystemdFS: &systemdFS,
	}
}
