#!/usr/bin/perl
# These arguments are operator configuration fixed in the forced command.
use strict;
use warnings;
use PVE::QemuConfig;
use PVE::QemuServer;
use PVE::Storage;

my ($id, $name, $uuid, $volume, $max_bytes) = @ARGV;
die "Invalid fixed capture configuration\n"
    unless @ARGV == 5 && $id =~ /^\d+$/ && $name =~ /^[A-Za-z0-9.-]+$/
    && $uuid =~ /^[0-9a-f-]{36}$/ && $volume =~ /^[\w-]+:vm-\Q$id\E-disk-\d+$/
    && $max_bytes =~ /^\d+$/ && $max_bytes > 0;
die "Unsupported capture command\n" unless ($ENV{SSH_ORIGINAL_COMMAND} // '') eq 'read-slot';

PVE::QemuConfig->lock_config($id, sub {
    my $conf = PVE::QemuConfig->load_config($id);
    PVE::QemuConfig->check_lock($conf);
    die "Slot identity changed\n" unless $conf->{name} eq $name && !$conf->{template}
        && ($conf->{smbios1} // '') =~ /(?:^|,)uuid=\Q$uuid\E(?:,|$)/
        && (split /,/, $conf->{scsi0} // '')[0] eq $volume;
    die "Capture requires a stopped slot\n" if PVE::QemuServer::check_running($id);
    my $disk = PVE::Storage::path(PVE::Storage::config(), $volume);
    die "Expected a raw block volume\n" unless -b $disk;
    open(my $size_cmd, '-|', '/usr/sbin/blockdev', '--getsize64', $disk) or die $!;
    my $size = <$size_cmd>;
    close($size_cmd) or die "Cannot read volume size\n";
    chomp($size);
    die "Volume exceeds capture budget\n" unless $size =~ /^\d+$/ && $size > 0 && $size <= $max_bytes;
    # Keep the PVE lock in the parent for the entire read. No guest filesystem
    # is mounted and the guest cannot start or reset through PVE during export.
    system('/usr/bin/timeout', '--kill-after=5s', '180', '/usr/bin/gzip', '-1', '-c', '--', $disk) == 0
        or die "Incomplete disk export\n";
});
