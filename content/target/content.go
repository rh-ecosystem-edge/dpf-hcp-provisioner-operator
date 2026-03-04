package target

import (
	"embed"

	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/ignition/content"
)

//go:embed files/*
var filesFS embed.FS

//go:embed systemd/*
var systemdFS embed.FS

func NewProvider() *content.EmbeddedProvider {
	f := func(name string) []byte { return content.EmbedFile(filesFS, "files/"+name) }

	return &content.EmbeddedProvider{
		Files: []content.FileDefinition{
			{
				Path:          "/usr/local/bin/devlink-activate.sh",
				Mode:          0755,
				ContentSource: f("devlink-activate.sh"),
			},
			{
				Path:          "/etc/mellanox/mlnx-bf.conf",
				Mode:          0644,
				ContentSource: f("mlnx-bf.conf"),
			},
			{
				Path:          "/etc/mellanox/mlnx-ovs.conf",
				Mode:          0644,
				ContentSource: f("mlnx-ovs.conf"),
			},
			{
				Path:          "/etc/NetworkManager/system-connections/pf0vf0.nmconnection",
				Mode:          0600,
				ContentSource: f("pf0vf0.nmconnection"),
			},
			{
				Path:          "/etc/NetworkManager/system-connections/br-comm-ch.nmconnection",
				Mode:          0600,
				ContentSource: f("br-comm-ch.nmconnection"),
			},
			{
				Path:          "/etc/NetworkManager/system-connections/oob_net0.nmconnection",
				Mode:          0600,
				ContentSource: f("oob_net0.nmconnection"),
			},
			{
				Path:          "/etc/crio/crio.conf.d/99-ulimits.conf",
				Mode:          0644,
				ContentSource: f("99-ulimits.conf"),
			},
			{
				Path:          "/etc/sysctl.d/98-dpunet.conf",
				Mode:          0644,
				ContentSource: f("98-dpunet.conf"),
			},
			{
				Path:          "/etc/modules-load.d/br_netfilter.conf",
				Mode:          0644,
				ContentSource: "data:,br_netfilter"},
			{
				Path:          "/etc/sysconfig/openvswitch",
				Mode:          0600,
				ContentSource: f("openvswitch"),
			},
			{
				Path:          "/usr/local/bin/dpf-configure-sfs.sh",
				Mode:          0755,
				ContentSource: f("dpf-configure-sfs.sh"),
			},
			{
				Path:          "/usr/local/bin/set-nvconfig-params.sh",
				Mode:          0755,
				ContentSource: f("set-nvconfig-params.sh"),
			},
		},
		SystemdFS: &systemdFS,
	}
}
