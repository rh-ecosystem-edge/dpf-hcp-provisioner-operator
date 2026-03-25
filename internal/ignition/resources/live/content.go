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

func NewProvider() *content.EmbeddedProvider {
	f := func(name string) []byte { return content.EmbedFile(filesFS, "files/"+name) }

	return &content.EmbeddedProvider{
		Files: []content.FileDefinition{
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
				Path: "/etc/dpf/identity",
				Mode: 0644,
				ContentSource: "data:," +
					"DPUName={{.DPUName}}" + nl +
					"DPUNamespace={{.DPUNamespace}}" + nl +
					"DPUUID={{.DPUUID}}" + nl,
			},
			{
				Path:          "/usr/local/bin/dpuagent-client.py",
				Mode:          0755,
				ContentSource: f("dpuagent-client.py"),
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
		},
		SystemdFS: &systemdFS,
	}
}
