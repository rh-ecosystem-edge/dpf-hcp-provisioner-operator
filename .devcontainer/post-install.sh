#!/bin/bash
set -x

SUDO=""
if [ "$(id -u)" != "0" ]; then
  SUDO="sudo"
fi

curl -Lo ./kind https://kind.sigs.k8s.io/dl/latest/kind-linux-amd64
chmod +x ./kind
${SUDO} mv ./kind /usr/local/bin/kind

KUBECTL_VERSION=$(curl -L -s https://dl.k8s.io/release/stable.txt)
curl -LO "https://dl.k8s.io/release/$KUBECTL_VERSION/bin/linux/amd64/kubectl"
chmod +x kubectl
${SUDO} mv kubectl /usr/local/bin/kubectl

# Rust toolchain comes with the image; add the components used by `make lint` and the musl
# target used by `make build`.
rustup component add clippy rustfmt
rustup target add "$(uname -m)-unknown-linux-musl"

docker network create -d=bridge --subnet=172.19.0.0/24 kind

kind version
docker --version
go version
cargo --version
kubectl version --client
