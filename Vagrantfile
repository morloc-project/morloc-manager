# -*- mode: ruby -*-
# vi: set ft=ruby :

# Vagrantfile for VM-based testing of morloc-manager
# 3 VMs: each gets Docker + Podman, per-persona user accounts,
# and morloc images pulled during provisioning.
#
# | VM     | Distro        | Primary concern          | Also tests                    |
# |--------|---------------|--------------------------|-------------------------------|
# | fedora | Fedora 40     | SELinux enforcing, cgv2  | Docker+Podman rootless/rootful|
# | ubuntu | Ubuntu 22.04  | AppArmor                 | Docker+Podman rootless/rootful|
# | debian | Debian 12     | cgroup v1                | Docker+Podman rootless/rootful|
#
# Each VM provisions 4 user accounts (one per persona) so that all personas
# can SSH in without resetting state between runs. Agents start in a dirty
# session with possible artifacts from past runs.
#
# Prerequisites:
#   vagrant plugin install vagrant-libvirt
#
# Usage:
#   vagrant up fedora                   # Start a single VM
#   vagrant ssh fedora                  # SSH into a VM
#   ./test/run-exploration.sh --vm fedora
#   vagrant destroy -f fedora           # Clean up

MORLOC_IMAGE = "ghcr.io/morloc-project/morloc/morloc-full:edge"

# Persona users provisioned on each VM. Each gets their own home directory,
# subuid/subgid ranges for rootless containers, and docker group membership.
PERSONA_USERS = {
  "newuser"       => { uid_start: 100000 },
  "developer"     => { uid_start: 200000 },
  "sysadmin"      => { uid_start: 300000 },
  "poweruser"     => { uid_start: 400000 },
  "mathematician" => { uid_start: 500000 },
}

# Shared provisioning script: creates persona users, installs engines, pulls images
PROVISION_USERS = <<-SHELL
  # Create persona user accounts
  #{PERSONA_USERS.map { |user, opts|
    <<~USR
      if ! id #{user} &>/dev/null; then
        useradd -m -s /bin/bash #{user}
      fi
      grep -q '^#{user}:' /etc/subuid || echo "#{user}:#{opts[:uid_start]}:65536" >> /etc/subuid
      grep -q '^#{user}:' /etc/subgid || echo "#{user}:#{opts[:uid_start]}:65536" >> /etc/subgid
      loginctl enable-linger #{user} || true
      usermod -aG docker #{user} 2>/dev/null || true
    USR
  }.join("\n")}

  # Also set up vagrant user for direct SSH
  usermod -aG docker vagrant 2>/dev/null || true
SHELL

Vagrant.configure("2") do |config|
  config.vm.provider :libvirt do |lv|
    lv.memory = 4096
    lv.cpus = 2
    lv.machine_virtual_size = 30
  end

  config.vm.synced_folder ".", "/vagrant", type: "rsync",
    rsync__exclude: [".git/", "findings/"],
    rsync__args: ["--verbose", "--archive", "--delete", "-z", "--copy-links"]

  # ---------- Fedora 40 ----------
  # Primary: SELinux enforcing, cgroup v2
  config.vm.define "fedora" do |node|
    node.vm.box = "bento/fedora-40"
    node.vm.provision "shell", inline: <<-SHELL
      set -e

      # Install Docker
      dnf install -y dnf-plugins-core || true
      dnf config-manager --add-repo https://download.docker.com/linux/fedora/docker-ce.repo 2>/dev/null || true
      dnf install -y docker-ce docker-ce-cli containerd.io || dnf install -y moby-engine
      systemctl enable --now docker

      # Install Podman and rootless dependencies
      dnf install -y podman fuse-overlayfs slirp4netns

      # Dev tools
      dnf install -y git ShellCheck

      #{PROVISION_USERS}

      # Pull morloc images on both engines
      docker pull #{MORLOC_IMAGE} || echo "WARNING: docker pull failed"
      podman pull #{MORLOC_IMAGE} || echo "WARNING: podman pull failed"

      # Verify SELinux is enforcing
      echo "SELinux status: $(getenforce)"

      # Verify cgroup v2
      if [ -f /sys/fs/cgroup/cgroup.controllers ]; then
        echo "cgroup v2 detected"
      fi
    SHELL
  end

  # ---------- Ubuntu 22.04 ----------
  # Primary: AppArmor
  config.vm.define "ubuntu" do |node|
    node.vm.box = "generic/ubuntu2204"
    node.vm.provision "shell", inline: <<-SHELL
      set -e
      export DEBIAN_FRONTEND=noninteractive

      apt-get update

      # Install Docker
      apt-get install -y ca-certificates curl gnupg
      install -m 0755 -d /etc/apt/keyrings
      if [ ! -f /etc/apt/keyrings/docker.gpg ]; then
        curl -fsSL https://download.docker.com/linux/ubuntu/gpg | gpg --dearmor -o /etc/apt/keyrings/docker.gpg
        chmod a+r /etc/apt/keyrings/docker.gpg
        echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.gpg] https://download.docker.com/linux/ubuntu $(. /etc/os-release && echo "$VERSION_CODENAME") stable" > /etc/apt/sources.list.d/docker.list
        apt-get update
      fi
      apt-get install -y docker-ce docker-ce-cli containerd.io || apt-get install -y docker.io
      systemctl enable --now docker

      # Install Podman
      apt-get install -y podman || true

      # Rootless dependencies
      apt-get install -y uidmap fuse-overlayfs slirp4netns || true

      # Dev tools
      apt-get install -y git shellcheck

      #{PROVISION_USERS}

      # Pull morloc images
      docker pull #{MORLOC_IMAGE} || echo "WARNING: docker pull failed"
      podman pull #{MORLOC_IMAGE} || echo "WARNING: podman pull failed"

      # Verify AppArmor
      aa-status || echo "AppArmor status check done"
    SHELL
  end

  # ---------- Debian 12 ----------
  # Primary: cgroup v1
  config.vm.define "debian" do |node|
    node.vm.box = "generic/debian12"
    node.vm.provision "shell", inline: <<-SHELL
      set -e
      export DEBIAN_FRONTEND=noninteractive

      apt-get update

      # Configure cgroup v1 via grub
      if ! grep -q "systemd.unified_cgroup_hierarchy=0" /etc/default/grub; then
        sed -i 's/GRUB_CMDLINE_LINUX_DEFAULT="\\(.*\\)"/GRUB_CMDLINE_LINUX_DEFAULT="\\1 systemd.unified_cgroup_hierarchy=0"/' /etc/default/grub
        update-grub
        echo "NOTE: cgroup v1 will be active after reboot"
        echo "Run: vagrant reload debian"
      fi

      # Install Docker
      apt-get install -y ca-certificates curl gnupg
      install -m 0755 -d /etc/apt/keyrings
      if [ ! -f /etc/apt/keyrings/docker.gpg ]; then
        curl -fsSL https://download.docker.com/linux/debian/gpg | gpg --dearmor -o /etc/apt/keyrings/docker.gpg
        chmod a+r /etc/apt/keyrings/docker.gpg
        echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.gpg] https://download.docker.com/linux/debian $(. /etc/os-release && echo "$VERSION_CODENAME") stable" > /etc/apt/sources.list.d/docker.list
        apt-get update
      fi
      apt-get install -y docker-ce docker-ce-cli containerd.io || apt-get install -y docker.io
      systemctl enable --now docker

      # Install Podman
      apt-get install -y podman || true

      # Rootless dependencies
      apt-get install -y uidmap fuse-overlayfs slirp4netns || true

      # Dev tools
      apt-get install -y git shellcheck

      #{PROVISION_USERS}

      # Pull morloc images
      docker pull #{MORLOC_IMAGE} || echo "WARNING: docker pull failed"
      podman pull #{MORLOC_IMAGE} || echo "WARNING: podman pull failed"

      # Report cgroup version
      if [ -f /sys/fs/cgroup/cgroup.controllers ]; then
        echo "cgroup v2 detected (will switch to v1 after reboot)"
      elif [ -d /sys/fs/cgroup/cpu ]; then
        echo "cgroup v1 detected"
      fi
    SHELL
  end
end
