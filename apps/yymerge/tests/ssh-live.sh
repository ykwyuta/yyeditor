#!/bin/sh
# Run only inside a disposable container: installs packages and creates a test user.
set -eu
: "${YYMERGE_TEST_EXE:?Set YYMERGE_TEST_EXE to the built Linux yymerge}"
test -x "$YYMERGE_TEST_EXE"
mkdir -p /results
apt-get update -qq
apt-get install -y -qq --no-install-recommends openssh-server
mkdir -p /run/sshd /tmp/yymerge-ssh-test /tmp/yymerge-settings/yyeditor
useradd -m -s /bin/sh yymergecheck
passwd -d yymergecheck >/dev/null
ssh-keygen -q -t ed25519 -N '' -f /tmp/yymerge-ssh-test/host
ssh-keygen -q -t ed25519 -N '' -f /tmp/yymerge-ssh-test/client
cat >/tmp/yymerge-ssh-test/sshd.conf <<EOF
Port 2222
ListenAddress 127.0.0.1
HostKey /tmp/yymerge-ssh-test/host
PidFile /tmp/yymerge-ssh-test/sshd.pid
AuthorizedKeysFile /tmp/yymerge-ssh-test/client.pub
PasswordAuthentication no
KbdInteractiveAuthentication no
UsePAM no
StrictModes no
Subsystem sftp internal-sftp
LogLevel VERBOSE
EOF
/usr/sbin/sshd -D -e -f /tmp/yymerge-ssh-test/sshd.conf >/results/sshd.log 2>&1 &
serverpid=$!
trap 'kill "$serverpid" 2>/dev/null || true' EXIT
for i in $(seq 1 100); do
  if grep -q 'Server listening' /results/sshd.log; then break; fi
  sleep 0.05
done
cat >/tmp/yymerge-settings/yyeditor/config.toml <<EOF
[remote]
read_ssh_config=false
read_ssh_known_hosts=false
[remote.host.integration]
hostname='127.0.0.1'
port=2222
user='yymergecheck'
identity_file='/tmp/yymerge-ssh-test/client'
EOF
printf '[127.0.0.1]:2222 ' >/tmp/yymerge-settings/yyeditor/known_hosts
cat /tmp/yymerge-ssh-test/host.pub >>/tmp/yymerge-settings/yyeditor/known_hosts
export XDG_CONFIG_HOME=/tmp/yymerge-settings
# A real 40 MiB SSH transfer, preserving bytes and using the shared host alias.
dd if=/dev/zero of=/tmp/yymerge-ssh-test/data.txt bs=1048576 count=40 status=none
"$YYMERGE_TEST_EXE" diff ssh://integration/tmp/yymerge-ssh-test/data.txt /tmp/yymerge-ssh-test/data.txt > /results/local-remote.txt
"$YYMERGE_TEST_EXE" large-diff ssh://integration/tmp/yymerge-ssh-test/data.txt ssh://integration/tmp/yymerge-ssh-test/data.txt > /results/remote-remote.txt
ssh-keygen -q -t ed25519 -N '' -f /tmp/yymerge-ssh-test/different
printf '[127.0.0.1]:2222 ' >/tmp/yymerge-settings/yyeditor/known_hosts
cat /tmp/yymerge-ssh-test/different.pub >>/tmp/yymerge-settings/yyeditor/known_hosts
set +e
"$YYMERGE_TEST_EXE" diff ssh://integration/tmp/yymerge-ssh-test/data.txt /tmp/yymerge-ssh-test/data.txt > /results/changed-key-out.txt 2>/results/changed-key-error.txt
status=$?
set -e
test "$status" = 2
printf 'PASS: local/remote and remote/remote 40 MiB over real SSH; changed host key rejected (exit %s)\n' "$status" >/results/result.txt
cat /results/result.txt
