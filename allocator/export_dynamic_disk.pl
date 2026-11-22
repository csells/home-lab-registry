#!/usr/bin/perl
# The operator pins the guest ID, storage directory and disk size in sudoers.
use strict;
use warnings;
use Cwd qw(abs_path);
use Errno qw(ENOENT);
use Fcntl qw(O_RDONLY O_NOFOLLOW);
use POSIX qw(dup2);
use JSON::PP qw(encode_json);
use PVE::QemuConfig;
use PVE::QemuServer;
use PVE::Storage;

my ($id, $store, $root, $bytes, $lease, $uuid, $generation, $volume) = @ARGV;
die "Invalid capture scope\n" unless (@ARGV == 4 || @ARGV == 8) && $id =~ /^[0-9]+$/ &&
    $store =~ /^[A-Za-z0-9-]+$/ && $root =~ m{^/[A-Za-z0-9_/-]+$} && $bytes =~ /^[1-9][0-9]+$/;

my $scfg = PVE::Storage::config();
my $spec = PVE::Storage::storage_config($scfg, $store);
die "Storage scope changed\n" unless $spec->{type} eq 'dir' && $spec->{path} eq $root &&
    (abs_path($root) // '') eq $root;

if (@ARGV == 4) {
    my $dir = "$root/images/$id";
    my @entries;
    if (lstat($dir)) {
        die "Unexpected scope directory\n" unless -d _ && (abs_path($dir) // '') eq $dir;
        opendir(my $scan, $dir) or die "Cannot inspect owned volumes\n";
        for my $name (readdir $scan) {
            next if $name eq '.' || $name eq '..';
            die "Unexpected scope volume\n" unless $name =~ /^vm-\Q$id\E-(?:disk-[0-9]+|cloudinit)\.raw$/ &&
                -f "$dir/$name" && (abs_path("$dir/$name") // '') eq "$dir/$name";
            push @entries, { volid => "$store:$id/$name" };
            die "Unexpected scope volume count\n" if @entries > 8;
        }
        closedir($scan);
    } elsif (!$!{ENOENT}) {
        die "Cannot inspect scoped storage directory\n";
    }
    print encode_json(\@entries), "\n";
    exit 0;
}

my $guid = qr/[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}/;
die "Invalid capture lease identity\n" unless
    $lease =~ /^[a-f0-9]{32}$/ && $uuid =~ /^$guid$/ && $generation =~ /^$guid$/ &&
    $volume =~ /^\Q$store:$id\/vm-$id-disk-\E[0-9]+\.raw$/;

PVE::QemuConfig->lock_config($id, sub {
    my $conf = PVE::QemuConfig->load_config($id);
    PVE::QemuConfig->check_lock($conf);
    die "Slot identity changed\n" unless ($conf->{name} // '') eq "lease-$lease"
        && !$conf->{template} && ($conf->{vmgenid} // '') eq $generation
        && ($conf->{smbios1} // '') =~ /(?:^|,)uuid=\Q$uuid\E(?:,|$)/
        && (split /,/, $conf->{scsi0} // '')[0] eq $volume;
    die "Capture requires a stopped slot\n" if PVE::QemuServer::check_running($id);

    my $path = PVE::Storage::path($scfg, $volume);
    my ($filename) = $volume =~ m{/(vm-[0-9]+-disk-[0-9]+\.raw)$};
    die "Volume outside enrolled directory\n" unless $filename &&
        $path eq "$root/images/$id/$filename" && (abs_path($path) // '') eq $path;
    sysopen(my $input, $path, O_RDONLY | O_NOFOLLOW) or die "Cannot open owned raw disk\n";
    die "Unexpected raw disk size\n" unless -f $input && (-s $input) == $bytes;

    # The PVE lock stays held until gzip has closed the pinned read-only handle.
    my $input_fd = dup2(fileno($input), 0);
    die "Cannot bind owned disk to capture input\n" unless defined($input_fd) && $input_fd == 0;
    system('/usr/bin/timeout', '--kill-after=5s', '180', '/usr/bin/gzip', '-1', '-c') == 0
        or die "Incomplete disk export\n";
});
