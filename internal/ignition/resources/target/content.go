package target

import (
	"bytes"
	"embed"
	"fmt"
	"net/url"
	"text/template"

	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/ignition/content"
)

//go:embed files/*
var filesFS embed.FS

//go:embed systemd/*
var systemdFS embed.FS

func NewProvider(zeroTrust bool) *content.EmbeddedProvider {
	f := func(name string) []byte { return content.EmbedFile(filesFS, "files/"+name) }

	p := &content.EmbeddedProvider{
		Files: []content.FileDefinition{
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
				ContentSource: "data:text/plain," + url.QueryEscape(string(f("pf0vf0.nmconnection"))),
			},
			{
				Path:          "/etc/NetworkManager/system-connections/br-comm-ch.nmconnection",
				Mode:          0600,
				ContentSource: f("br-comm-ch.nmconnection"),
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
				Path:          "/etc/systemd/system/chrony-wait.service.d/override.conf",
				Mode:          0644,
				ContentSource: f("10-chrony-wait-timeout.conf"),
			},
			{
				Path:          "/etc/systemd/system/NetworkManager-wait-online.service.d/override.conf",
				Mode:          0644,
				ContentSource: f("10-nm-wait-online-unstrict.conf"),
			},
			{
				Path:          "/etc/yum.repos.d/agentrepo.repo",
				Mode:          0644,
				ContentSource: f("agentrepo.repo"),
			},
			{
				Path:          "/etc/systemd/system/machine-config-daemon-firstboot.service.d/10-mcd-firstboot-dpuagent.conf",
				Mode:          0644,
				ContentSource: f("10-mcd-firstboot-dpuagent.conf"),
			},
			{
				Path:          "/etc/systemd/system/machine-config-daemon-pull.service.d/10-require-setup-vfs.conf",
				Mode:          0644,
				ContentSource: f("10-require-setup-vfs.conf"),
			},
			{
				Path:          "/usr/local/bin/install-dpu-agent.sh",
				Mode:          0755,
				ContentSource: f("install-dpu-agent.sh"),
			},
			{
				Path:          "/usr/local/bin/dpu-fw-upgrade.sh",
				Mode:          0755,
				ContentSource: f("dpu-fw-upgrade.sh"),
			},
			{
				Path:          "/usr/local/bin/wait-for-sfs.sh",
				Mode:          0755,
				ContentSource: f("wait-for-sfs.sh"),
			},
			{
				Path:          "/usr/local/bin/tmfifo-agent-link.sh",
				Mode:          0755,
				ContentSource: f("tmfifo-agent-link.sh"),
			},
			{
				Path:          "/usr/local/bin/report-machineosurl.py",
				Mode:          0755,
				ContentSource: f("report-machineosurl.py"),
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
				Path:          "/etc/tmpfiles.d/99-hugetlbfs-dpf.conf",
				Mode:          0644,
				ContentSource: f("tmpfiles-hugetlbfs-dpf.conf"),
			},
			{
				Path:          "/etc/openshift/kubelet.conf.d/kubelet-dpf-override.conf",
				Mode:          0644,
				ContentSource: f("kubelet-dpf-override.conf"),
			},
			{
				Path:          "/usr/local/bin/pf-monitor.sh",
				Mode:          0755,
				ContentSource: f("pf-monitor.sh"),
			},
		},
		Links: []content.LinkDefinition{
			{
				Path:   "/etc/localtime",
				Target: "/usr/share/zoneinfo/UTC",
			},
		},
		SystemdFS: &systemdFS,
	}

	if zeroTrust {
		p.SkipUnits = []string{
			"pf-monitor.service",
			"report-machineosurl.service",
			"tmfifo-agent-link.service",
			"bfupsignal.service",
			"ovs-vf-recovery.service",
		}
		excludedFiles := map[string]bool{
			"/usr/local/bin/pf-monitor.sh":          true,
			"/usr/local/bin/report-machineosurl.py": true,
			"/etc/yum.repos.d/agentrepo.repo":       true,
			"/usr/local/bin/tmfifo-agent-link.sh":   true,
		}
		filtered := make([]content.FileDefinition, 0, len(p.Files))
		for _, f := range p.Files {
			if !excludedFiles[f.Path] {
				filtered = append(filtered, f)
			}
		}
		p.Files = filtered
	}

	return p
}

// renderServiceUnit renders a Go-templated systemd unit file, passing ZeroTrust as template data.
func renderServiceUnit(tmplFile, unitName string, zeroTrust bool) (string, string, error) {
	tmplBytes := content.EmbedFile(filesFS, "files/"+tmplFile)
	tmpl, err := template.New(unitName).Parse(string(tmplBytes))
	if err != nil {
		return "", "", fmt.Errorf("failed to parse %s template: %w", unitName, err)
	}

	data := struct{ ZeroTrust bool }{ZeroTrust: zeroTrust}
	var buf bytes.Buffer
	if err := tmpl.Execute(&buf, data); err != nil {
		return "", "", fmt.Errorf("failed to render %s template: %w", unitName, err)
	}

	return unitName, buf.String(), nil
}

func RenderDPUAgentServiceUnit(zeroTrust bool) (string, string, error) {
	return renderServiceUnit("dpu-agent.service.tmpl", "dpu-agent.service", zeroTrust)
}

func RenderInstallDPUAgentServiceUnit(zeroTrust bool) (string, string, error) {
	return renderServiceUnit("install-dpu-agent.service.tmpl", "install-dpu-agent.service", zeroTrust)
}

func RenderDPUFWUpgradeServiceUnit(zeroTrust bool) (string, string, error) {
	return renderServiceUnit("dpu-fw-upgrade.service.tmpl", "dpu-fw-upgrade.service", zeroTrust)
}

func RenderSetupVFSDevlinkServiceUnit(zeroTrust bool) (string, string, error) {
	return renderServiceUnit("setup-vfs-devlink.service.tmpl", "setup-vfs-devlink.service", zeroTrust)
}
