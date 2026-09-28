#!/usr/bin/env bash
# Starts throwaway OpenSSH servers for NexSSH's integration tests (Linux, needs root).
#
#   sudo scripts/test-sshd.sh            # start
#   eval "$(sudo scripts/test-sshd.sh env)"   # print variables for `cargo test`
#   sudo scripts/test-sshd.sh stop
#
# Servers:
#   127.0.0.1:2222  password + public key (no PAM), TCP forwarding allowed
#   127.0.0.1:2223  keyboard-interactive only (PAM)
set -euo pipefail

DIR="${NEXSSH_TEST_DIR:-/tmp/nexssh-sshd}"
USER_NAME="nexssh-test"
PASSWORD="nexssh-pass-42"
PASSPHRASE="nexssh-phrase"

print_env() {
  cat <<VARS
export NEXSSH_TEST_HOST=127.0.0.1
export NEXSSH_TEST_PORT=2222
export NEXSSH_TEST_KBD_PORT=2223
export NEXSSH_TEST_USER=$USER_NAME
export NEXSSH_TEST_PASSWORD=$PASSWORD
export NEXSSH_TEST_KEY=$DIR/client/id_ed25519
export NEXSSH_TEST_ENC_KEY=$DIR/client/id_ed25519_enc
export NEXSSH_TEST_RSA_KEY=$DIR/client/id_rsa
export NEXSSH_TEST_PASSPHRASE=$PASSPHRASE
VARS
}

case "${1:-start}" in
  env) print_env; exit 0 ;;
  stop)
    for f in "$DIR"/sshd-*.pid; do [ -f "$f" ] && kill "$(cat "$f")" 2>/dev/null || true; done
    exit 0 ;;
esac

SSHD="$(command -v sshd || echo /usr/sbin/sshd)"
mkdir -p "$DIR/client" /run/sshd
chmod 755 "$DIR"

if ! id "$USER_NAME" >/dev/null 2>&1; then
  useradd -m -s /bin/bash "$USER_NAME"
fi
echo "$USER_NAME:$PASSWORD" | chpasswd

[ -f "$DIR/ssh_host_ed25519_key" ] || ssh-keygen -q -t ed25519 -N "" -f "$DIR/ssh_host_ed25519_key"
[ -f "$DIR/ssh_host_rsa_key" ] || ssh-keygen -q -t rsa -b 2048 -N "" -f "$DIR/ssh_host_rsa_key"
[ -f "$DIR/client/id_ed25519" ] || ssh-keygen -q -t ed25519 -N "" -C "nexssh plain" -f "$DIR/client/id_ed25519"
[ -f "$DIR/client/id_ed25519_enc" ] || ssh-keygen -q -t ed25519 -N "$PASSPHRASE" -C "nexssh encrypted" -f "$DIR/client/id_ed25519_enc"
[ -f "$DIR/client/id_rsa" ] || ssh-keygen -q -t rsa -b 2048 -N "" -C "nexssh rsa" -f "$DIR/client/id_rsa"
chmod 644 "$DIR"/client/*.pub

HOME_DIR="$(getent passwd "$USER_NAME" | cut -d: -f6)"
install -d -m 700 -o "$USER_NAME" "$HOME_DIR/.ssh"
cat "$DIR"/client/*.pub > "$HOME_DIR/.ssh/authorized_keys"
chown "$USER_NAME" "$HOME_DIR/.ssh/authorized_keys"
chmod 600 "$HOME_DIR/.ssh/authorized_keys"

common() {
  cat <<CONF
ListenAddress 127.0.0.1
HostKey $DIR/ssh_host_ed25519_key
HostKey $DIR/ssh_host_rsa_key
AllowUsers $USER_NAME
AllowTcpForwarding yes
PermitTTY yes
LogLevel VERBOSE
CONF
}

{ common; cat <<CONF
Port 2222
PidFile $DIR/sshd-2222.pid
UsePAM no
PasswordAuthentication yes
KbdInteractiveAuthentication no
PubkeyAuthentication yes
CONF
} > "$DIR/sshd-2222.conf"

{ common; cat <<CONF
Port 2223
PidFile $DIR/sshd-2223.pid
UsePAM yes
PasswordAuthentication no
KbdInteractiveAuthentication yes
PubkeyAuthentication no
CONF
} > "$DIR/sshd-2223.conf"

for port in 2222 2223; do
  pidfile="$DIR/sshd-$port.pid"
  if [ -f "$pidfile" ] && kill -0 "$(cat "$pidfile")" 2>/dev/null; then continue; fi
  "$SSHD" -f "$DIR/sshd-$port.conf" -E "$DIR/sshd-$port.log"
done
sleep 0.5
echo "sshd running on 127.0.0.1:2222 and :2223 (logs in $DIR)" >&2
print_env
