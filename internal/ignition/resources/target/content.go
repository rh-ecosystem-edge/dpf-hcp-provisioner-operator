package target

import (
	"embed"

	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/ignition/content"
)

//go:embed files/*
var filesFS embed.FS

//go:embed systemd/*
var systemdFS embed.FS

func NewProvider(zeroTrust bool) *content.EmbeddedProvider {
	f := func(name string) []byte { return content.EmbedFile(filesFS, "files/"+name) }
	nl := "%0A"

	extraArgs := ""
	if zeroTrust {
		extraArgs = " --zero-trust-mode" +
			" --bootstrap-kubeconfig=/var/lib/dpf/dpuagent/bootstrap-kubeconfig"
	}

	dpuAgentService := "data:," +
		"[Unit]" + nl +
		"Description=DPF DPU Agent - Provisioning and Configuration" + nl +
		"After=tmfifo-agent-link.service install-dpu-agent.service dpu-fw-upgrade.service" + nl +
		"Before=nodeip-configuration.service kubelet-dependencies.target ovs-configuration.service" + nl +
		"Requires=tmfifo-agent-link.service" + nl +
		"Wants=install-dpu-agent.service dpu-fw-upgrade.service" + nl +
		"ConditionPathExists=/etc/mlnx-release" + nl +
		"ConditionPathExists=/usr/local/bin/dpu-agent" + nl +
		nl +
		"[Service]" + nl +
		"Type=simple" + nl +
		"EnvironmentFile=/etc/dpf/environment" + nl +
		"ExecStart=/usr/local/bin/dpu-agent" +
		" --dpu-name $DPUName" +
		" --dpu-namespace $DPUNamespace" +
		" --dpu-uid $DPUUID" +
		" --dpuflavor /etc/dpf/dpuflavor.yaml" +
		" --skip-containerd-config" +
		" --skip-dns-config" +
		" --skip-kernel-cmd-line" +
		" --skip-network-config" +
		" --skip-remove-builtin-kubelet" +
		" --skip-configure-kubelet" +
		" --skip-start-kubelet" +
		" --skip-ovs-raw-script" +
		" --kubeadm-secret-name=unused" +
		extraArgs + nl +
		"Restart=on-failure" + nl +
		"RestartSec=5" + nl +
		nl +
		"[Install]" + nl +
		"WantedBy=multi-user.target" + nl

	return &content.EmbeddedProvider{
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
				ContentSource: f("pf0vf0.nmconnection"),
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
			{
				Path:          "/etc/systemd/system/dpu-agent.service",
				Mode:          0644,
				ContentSource: dpuAgentService,
			},
		},
		SystemdFS: &systemdFS,
	}
}
