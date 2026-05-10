#!/usr/bin/env bash
set -euo pipefail

archive_path="${1:?archive path is required}"
version="${2:?version is required}"
service_user="${3:-game-server}"
service_name="${4:-game-server}"
bind_addr="${5:-0.0.0.0:7777}"
auth_mode="${6:-offline}"
restart_notice_seconds="${7:-15}"

sudo_cmd=()
if [[ "${EUID}" -ne 0 ]]; then
  sudo_cmd=(sudo)
fi

as_root() {
  "${sudo_cmd[@]}" "$@"
}

as_service_user() {
  if [[ "${EUID}" -eq 0 ]]; then
    runuser -u "${service_user}" -- "$@"
  else
    sudo -u "${service_user}" "$@"
  fi
}

home_dir="/home/${service_user}"
install_dir="${home_dir}/${service_name}"
release_dir="${install_dir}/releases/${version}"
current_link="${install_dir}/current"
data_dir="${install_dir}/data"
admin_socket="/run/${service_name}/admin.sock"
world_path="${data_dir}/world.json"
unit_path="/etc/systemd/system/${service_name}.service"
shutdown_reason="Server is updating to ${version}. Please download the latest client before reconnecting."

if ! command -v apt-get >/dev/null 2>&1; then
  echo "This deploy script currently expects an apt-based server." >&2
  exit 1
fi

as_root apt-get update
as_root env DEBIAN_FRONTEND=noninteractive apt-get upgrade -y
runtime_packages=(
  ca-certificates
  tar
  libasound2t64
  libudev1
  libwayland-client0
  libwayland-cursor0
  libwayland-egl1
  libx11-6
  libx11-xcb1
  libxcb1
  libxcb-randr0
  libxcb-xfixes0
  libxcursor1
  libxi6
  libxkbcommon0
  libxkbcommon-x11-0
  libxrandr2
  libxrender1
)

if ! apt-cache show libasound2t64 >/dev/null 2>&1; then
  runtime_packages=("${runtime_packages[@]/libasound2t64/libasound2}")
fi

as_root env DEBIAN_FRONTEND=noninteractive apt-get install -y "${runtime_packages[@]}"

if ! getent group "${service_user}" >/dev/null; then
  as_root groupadd --system "${service_user}"
fi

if ! id -u "${service_user}" >/dev/null 2>&1; then
  as_root useradd \
    --system \
    --gid "${service_user}" \
    --home-dir "${home_dir}" \
    --create-home \
    --shell /usr/sbin/nologin \
    "${service_user}"
fi

as_root install -d -m 0750 -o "${service_user}" -g "${service_user}" "${home_dir}"
as_root install -d -m 0750 -o "${service_user}" -g "${service_user}" "${install_dir}"
as_root install -d -m 0750 -o "${service_user}" -g "${service_user}" "${install_dir}/releases"
as_root install -d -m 0750 -o "${service_user}" -g "${service_user}" "${data_dir}"

if [[ -e "${release_dir}" ]]; then
  release_dir="${release_dir}-$(date +%s)"
fi

as_root install -d -m 0750 -o "${service_user}" -g "${service_user}" "${release_dir}"
as_root tar -xzf "${archive_path}" -C "${release_dir}"
as_root chown -R "${service_user}:${service_user}" "${release_dir}"
as_root chmod 0750 "${release_dir}/game"
as_root ln -sfn "${release_dir}" "${current_link}"
as_root chown -h "${service_user}:${service_user}" "${current_link}"

unit_tmp="$(mktemp)"
cat > "${unit_tmp}" <<EOF
[Unit]
Description=Game Dedicated Server
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=${service_user}
Group=${service_user}
WorkingDirectory=${install_dir}
RuntimeDirectory=${service_name}
RuntimeDirectoryMode=0750
UMask=007
ExecStart=${current_link}/game server --bind ${bind_addr} --auth ${auth_mode} --world ${world_path} --admin-socket ${admin_socket}
ExecStop=/bin/sh -c '${current_link}/game admin --socket ${admin_socket} announce "${shutdown_reason}" || true'
ExecStop=/bin/sleep 3
ExecStop=/bin/sh -c '${current_link}/game admin --socket ${admin_socket} shutdown --reason "${shutdown_reason}" || true'
ExecStop=/bin/sleep 5
KillSignal=SIGINT
TimeoutStopSec=45
Restart=on-failure
RestartSec=5
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=full
ReadWritePaths=${install_dir} /run/${service_name}

[Install]
WantedBy=multi-user.target
EOF

as_root install -m 0644 -o root -g root "${unit_tmp}" "${unit_path}"
rm -f "${unit_tmp}"
as_root systemctl daemon-reload
as_root systemctl enable "${service_name}"

announce() {
  local message="$1"
  if [[ -x "${current_link}/game" ]]; then
    as_service_user "${current_link}/game" admin --socket "${admin_socket}" announce "${message}" || true
  fi
}

if as_root systemctl is-active --quiet "${service_name}"; then
  announce "Deploying ${version}. Server will restart in ${restart_notice_seconds} seconds. Please disconnect, download the latest client, then reconnect after it is back online."
  sleep "${restart_notice_seconds}"
  as_root systemctl stop "${service_name}"
fi

as_root systemctl start "${service_name}"

for _ in {1..20}; do
  if [[ -S "${admin_socket}" ]]; then
    break
  fi
  sleep 1
done

announce "Server is back online with ${version}. Please download the latest client before reconnecting."
as_root systemctl --no-pager --full status "${service_name}"
