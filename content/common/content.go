package common

import (
	"embed"

	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/ignition/content"
)

//go:embed files/*
var filesFS embed.FS

func NewProvider() *content.EmbeddedProvider {
	f := func(name string) []byte { return content.EmbedFile(filesFS, "files/"+name) }

	return &content.EmbeddedProvider{
		Files: []content.FileDefinition{
			{
				Path:          "/etc/systemd/network/10-tmfifo_net.link",
				Mode:          0644,
				ContentSource: f("10-tmfifo_net.link"),
			},
			{
				Path:          "/etc/NetworkManager/system-connections/tmfifo_net0.nmconnection",
				Mode:          0600,
				ContentSource: f("tmfifo_net0.nmconnection"),
			},
			{
				Path:          "/usr/local/bin/bflog.sh",
				Mode:          0755,
				ContentSource: f("bflog.sh"),
			},
			{
				Path:          "/usr/local/bin/bfupsignal.sh",
				Mode:          0755,
				ContentSource: f("bfupsignal.sh"),
			},
		},
	}
}
