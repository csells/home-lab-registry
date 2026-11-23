set -eu
umask 077
marker="$HOME/.lease-prepared-ssh"
if [ ! -f "$marker" ]; then
    /usr/bin/sudo -n /bin/sh -ec '
        config=$(/usr/bin/mktemp /etc/ssh/sshd_lease_config.XXXXXX)
        trap '\''/bin/rm -f -- "$config"'\'' EXIT
        {
            /usr/bin/printf "%s\n" "PasswordAuthentication no" "KbdInteractiveAuthentication no" "PermitRootLogin no"
            /bin/cat /etc/ssh/sshd_config
        } > "$config"
        /usr/sbin/sshd -t -f "$config"
        /bin/chmod 644 "$config"
        /bin/mv "$config" /etc/ssh/sshd_config
        /bin/rm -f -- /etc/ssh/ssh_host_ed25519_key /etc/ssh/ssh_host_ed25519_key.pub
        /usr/bin/ssh-keygen -q -t ed25519 -N "" -f /etc/ssh/ssh_host_ed25519_key
        /usr/sbin/sshd -t
    '
    : > "$marker"
fi
/usr/bin/sudo -n /bin/cat /etc/ssh/ssh_host_ed25519_key.pub
