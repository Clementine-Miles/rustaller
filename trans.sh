#!/bin/ash
# Personal Linux-only derivative of bin456789/reinstall; see docs/TRIMMING.md.
# shellcheck shell=dash
# shellcheck disable=SC2086,SC3047,SC3036,SC3010,SC3001,SC3060,SC3015
# alpine use by default busybox ash
# Notice bash and ash The following statements have different results
# [[ a = '*a' ]] && echo 1

# After an error occurs, the operation will stop and the login interface will be entered to prevent loss of connection.
set -eE

# used to judge reinstall.sh and trans.sh Is it compatible
# shellcheck disable=SC2034
SCRIPT_VERSION=PERSONAL-LINUX-ONLY-20261119-04
# Filled by extract_env_from_cmdline from the finalos_* kernel arguments.
releasever=${releasever:-}
codename=${codename:-}

TRUE=0
FALSE=1
EFI_UUID=C12A7328-F81F-11D2-BA4B-00A0C93EC93B

error() {
    color='\e[31m'
    plain='\e[0m'
    echo -e "${color}***** ERROR *****${plain}" >&2
    echo -e "${color}$*${plain}" >&2
}

info() {
    color='\e[32m'
    plain='\e[0m'
    local msg

    if [ "$1" = false ]; then
        shift
        msg=$*
    else
        msg=$(echo "$*" | to_upper)
    fi

    echo -e "${color}***** $msg *****${plain}" >&2
}

warn() {
    color='\e[33m'
    plain='\e[0m'
    echo -e "${color}Warning: $*${plain}" >&2
}

error_and_exit() {
    error "$@"

    if is_have_cmd sudo; then
        sudo_='sudo '
    elif is_have_cmd doas; then
        sudo_='doas '
    else
        sudo_=
    fi

    echo "Run '$sudo_/trans.sh' to retry." >&2
    echo "Run '$sudo_/trans.sh alpine' to install Alpine Linux instead." >&2

    # Unlock and allow users to log in to handle failures
    # passwd -u "$username" >/dev/null

    # No need, because alpine locked accounts cannot log in ssh
    # So it won’t lock

    exit 1
}

trap_err() {
    line_no=$1
    ret_no=$2

    error_and_exit "$(
        echo "Line $line_no return $ret_no"
        if [ -f "/trans.sh" ]; then
            sed -n "$line_no"p /trans.sh
        fi
    )"
}

is_run_from_locald() {
    [[ "$0" = "/etc/local.d/*" ]]
}

# reinstall.sh There is the same method add_community_repo_for_alpine
add_community_repo() {
    local ver mirror

    # Check the original first repo Yes or no edge or latest-stable
    if grep -q "^http.*/edge/main$" /etc/apk/repositories; then
        ver=edge
    elif grep -q "^http.*/latest-stable/main$" /etc/apk/repositories; then
        ver=latest-stable
    else
        ver=v$(cut -d. -f1,2 </etc/alpine-release)
    fi

    if ! grep -q "^http.*/$ver/community$" /etc/apk/repositories; then
        mirror=$(grep '^http.*/main$' /etc/apk/repositories | sed 's,/[^/]*/main$,,' | head -1)
        echo $mirror/$ver/community >>/etc/apk/repositories
    fi
}

# Sometimes the download fails due to network problems, causing the script to interrupt
# So need to try again
apk() {
    retry 5 command apk "$@" >&2
}

show_url_in_args() {
    while [ $# -gt 0 ]; do
        case "$1" in
        [Hh][Tt][Tt][Pp][Ss]://* | [Hh][Tt][Tt][Pp]://* | [Mm][Aa][Gg][Nn][Ee][Tt]:*) echo "$1" ;;
        esac
        shift
    done
}

killall() {
    # killall It’s asynchronous, please wait.
    local ret=0
    if ! command killall "$@"; then
        ret=$?
    fi
    sleep 5
    return $ret
}

# In no setting set +o pipefail In case of limit download size：
# retry 5 command wget | head -c 1048576 will trigger retry，download 5 Second-rate
# command wget "$@" --tries=5 | head -c 1048576 will not trigger wget Comes with it retry，download only 1 Second-rate
wget() {
    show_url_in_args "$@" >&2
    if command wget 2>&1 | grep -q BusyBox; then
        # busybox wget No retry function
        # It seems to never time out by default
        retry 5 command wget "$@" -T 10
    else
        # Original wget Comes with retry function
        command wget --tries=5 --progress=bar:force "$@"
    fi
}

is_have_cmd() {
    # command -v Including methods in scripts
    is_have_cmd_on_disk / "$1"
}

is_have_cmd_on_disk() {
    local os_dir=$1
    local cmd=$2

    for bin_dir in /bin /sbin /usr/bin /usr/sbin; do
        if [ -f "$os_dir$bin_dir/$cmd" ]; then
            return
        fi
    done
    return 1
}

is_num() {
    echo "$1" | grep -Exq '[0-9]*\.?[0-9]*'
}

retry() {
    local max_try=$1
    shift

    if is_num "$1"; then
        local interval=$1
        shift
    else
        local interval=5
    fi

    local i
    for i in $(seq $max_try); do
        if "$@"; then
            return
        else
            ret=$?
            # wget -O- | grep -m1 After success, the pipeline will be closed early, resulting in 141 error
            # This is expected behavior and therefore needs to be excluded
            if [ $ret -eq 141 ]; then
                return
            fi
            if [ $i -ge $max_try ]; then
                return $ret
            fi
            sleep $interval
        fi
    done
}

create_alpine_rootfs() {
    local os_dir=$1
    local init_now=${2:-false}

    # Copy the current system /etc/apk folder
    mkdir -p "$os_dir"
    cp -a --parents /etc/apk "$os_dir"
    rm -f "$os_dir/etc/apk/world"

    # Install alpine
    apk add --root "$os_dir" --initdb \
        alpine-base openssl ca-certificates

    if $init_now; then
        cp_resolv_conf "$os_dir"
        mount_pseudo_fs "$os_dir"
    fi
}

create_alpine_rootfs_with_arch_install_scripts() {
    local os_dir=$1
    local init_now=${2:-false}
    local parent_os_dir=$3

    create_alpine_rootfs "$os_dir" $init_now

    # Will alpine-base dependency writing world，Delete again alpine-base alpine-conf
    # --installed --depends ordering must be preserved
    # Do not add --installed will display both the installed version and the latest version
    alpine_base_depends=$(chroot "$os_dir" apk info --installed --depends alpine-base | sed '/depends on:/d')
    chroot "$os_dir" apk add $alpine_base_depends
    chroot "$os_dir" apk del alpine-base alpine-conf
    chroot "$os_dir" apk add arch-install-scripts

    if [ -n "$parent_os_dir" ]; then
        mkdir -p "$os_dir/parent"
        mount --rbind "$parent_os_dir" "$os_dir/parent"
    fi
}

remove_alpine_rootfs() {
    local os_dir=$1

    umount_pseudo_fs "$os_dir"
    rm -rf "$os_dir"
}

download() {
    local url=$1 path=$2
    aria2c "$url" -d "$(dirname "$path")" -o "$(basename "$path")" -U curl/7.54.1 ||
        error_and_exit "Failed to download $url"
}

update_part() {
    sleep 1
    sync
    sleep 1

    # partprobe
    # It will report if there is a partition being mounted. Resource busy error
    if is_have_cmd partprobe; then
        partprobe /dev/$xda 2>/dev/null || true
        sleep 1
    fi

    # partx
    # https://access.redhat.com/solutions/199573
    if is_have_cmd partx; then
        partx -u /dev/$xda
        sleep 1
    fi

    # mdev
    # mdev will not be deleted /dev/disk/ old partition, so delete it manually
    # if rm -rf It’s just the right time mdev Creating a link，rm -rf Will report an error Directory not empty
    # So stop it first mdev Serve
    # Also delete /dev/$xda*?
    ensure_service_stopped mdev
    sleep 1
    # even if it stops mdev，Sometimes I will report Directory not empty，So add retry
    retry 5 rm -rf /dev/disk/*

    # Not mounted modloop will prompt
    # modprobe: can't change directory to '/lib/modules': No such file or directory
    # Therefore, it is forced not to display the above prompt
    mdev -sf 2>/dev/null
    sleep 1
    ensure_service_started mdev 2>/dev/null
    sleep 1
}

is_efi() {
    if [ -n "$force_boot_mode" ]; then
        [ "$force_boot_mode" = efi ]
    else
        [ -d /sys/firmware/efi/ ]
    fi
}

setup_websocketd() {
    apk add websocketd
    apk add coreutils

    mkdir -p /tmp/web
    echo 'Wrong Path' >/tmp/web/index.html
    # shellcheck disable=SC2154
    wget $confhome/logviewer.html -O /tmp/web$web_path

    killall -q websocketd || true
    # websocketd when encountering \n Just push it, so it needs to be converted \r for \n
    websocketd --port "$web_port" --loglevel=fatal --staticdir=/tmp/web \
        stdbuf -oL -eL \
        sh -c "if [ \"\$PATH_INFO\" = \"$web_path\" ]; then tail -fn+0 /reinstall.log | tr '\r' '\n' | grep -Fiv -e password -e token; fi" &
}

get_approximate_ram_size() {
    # lsmem requires util-linux

    if [ -z $ram_size ]; then
        ram_size=$(free -m | awk '{print $2}' | sed -n '2p')
    fi

    echo "$ram_size"
}

setup_web_if_enough_ram() {
    total_ram=$(get_approximate_ram_size)
    # 512Memory is installed
    if [ "$total_ram" -ge 400 ]; then
        setup_websocketd
    fi
}

get_ttys() {
    local prefix=${1:-}
    local tty_root=/dev
    local is_for_cmdline=false
    local is_first=true
    local tty

    # the last one tty It’s the Lord tty，Display the most complete information
    # hytron have ttyS0 but cannot be written to; used for cmdline cannot exclude non-existent tty
    if [ "$prefix" = "console=" ]; then
        is_for_cmdline=true
    fi

    for tty in ttyS0 tty0; do
        if { [ -c "$tty_root/$tty" ] && stty -g -F "$tty_root/$tty" >/dev/null 2>&1; } ||
            { $is_for_cmdline && ! [ -c "$tty_root/$tty" ]; }; then
            if $is_first; then
                is_first=false
            else
                printf " "
            fi

            printf "%s" "$prefix$tty"
            if [ "$is_for_cmdline" = true ] && [ "$tty" = ttyS0 ]; then
                printf ",115200n8"
            fi
        fi
    done
}

find_xda() {
    # Run the script again after an error occurs. The hard disk may have been formatted and the partition table recorded previously id invalid
    # So find xda to save later xda arrive /configs/xda

    # Read previously saved
    if xda=$(get_config xda 2>/dev/null) && [ -n "$xda" ]; then
        return
    fi

    # prevent $main_disk is empty
    if [ -z "$main_disk" ]; then
        error_and_exit "cmdline main_disk is empty."
    fi

    # busybox fdisk/lsblk/blkid Don’t show mbr partition table id
    # The following tools are available：
    # fdisk exist util-linux-misc inside, consuming significant space
    # sfdisk Small footprint
    # lsblk
    # blkid

    tool=sfdisk

    is_have_cmd $tool && need_install_tool=false || need_install_tool=true
    if $need_install_tool; then
        apk add $tool
    fi

    if [ "$tool" = sfdisk ]; then
        # sfdisk
        for disk in $(get_all_disks); do
            if sfdisk --disk-id "/dev/$disk" | sed 's/0x//' | grep -ix "$main_disk"; then
                xda=$disk
                break
            fi
        done
    else
        # lsblk
        xda=$(lsblk --nodeps -rno NAME,PTUUID | grep -iw "$main_disk" | awk '{print $1}')
    fi

    if [ -n "$xda" ]; then
        set_config xda "$xda"
    else
        error_and_exit "Could not find xda: $main_disk"
    fi

    if $need_install_tool; then
        apk del $tool
    fi
}

get_all_disks() {
    # shellcheck disable=SC2010
    ls /sys/block/ | grep -Ev '^(loop|sr|nbd)'
}

extract_env_from_cmdline() {
    # extract finalos/extra to variable
    for prefix in finalos extra; do
        while read -r line; do
            if [ -n "$line" ]; then
                key=$(echo $line | cut -d= -f1)
                value=$(echo $line | cut -d= -f2-)
                eval "$key='$value'"
            fi
        done < <(xargs -n1 </proc/cmdline | grep "^${prefix}_" | sed "s/^${prefix}_//")
    done

    # Set default value if blank
    username=${username:-root}
    ssh_port=${ssh_port:-22}
    web_port=${web_port:-80}
}

ensure_service_started() {
    local service=$1

    if ! rc-service -q "$service" start; then
        for i in $(seq 10); do
            if [ "$service" = modloop ]; then
                # avoid sometimes modloop Incomplete download leads to error
                # * Failed to verify signature of !
                # mount: mounting /dev/loop0 on /.modloop failed: Invalid argument
                rm -f /lib/modloop-lts /lib/modloop-virt
            fi
            if rc-service -q "$service" start; then
                return
            fi
            sleep 5
        done
        error_and_exit "Failed to start $service."
    fi
}

ensure_service_stopped() {
    local service=$1

    if ! retry 10 5 rc-service -q "$service" stop; then
        error_and_exit "Failed to stop $service."
    fi
}

mod_motd() {
    # After installation alpine To restore the default
    # After automatic installation fails, manual installation may be required. alpine，So no need to judge $distro
    file=/etc/motd
    if ! [ -e $file.orig ]; then
        cp $file $file.orig
        # shellcheck disable=SC2016
        echo "mv "\$mnt$file.orig" "\$mnt$file"" |
            insert_into_file "$(which setup-disk)" before 'cleanup_chroot_mounts "\$mnt"'

        cat <<EOF >$file
Reinstalling...
To view logs run:
tail -fn+1 /reinstall.log
EOF
    fi
}

umount_all() {
    dirs="/mnt /os /iso /root"
    regex=$(echo "$dirs" | sed 's, ,|,g')
    if mounts=$(mount | grep -Ew "on $regex" | awk '{print $3}' | tac); then
        for mount in $mounts; do
            echo "umount $mount"
            umount $mount
        done
    fi
}

# Maybe the script is not run for the first time, so clean up the previous remnants first.
clear_previous() {
    if is_have_cmd vgchange; then
        umount -R /os || true
        vgchange -an
        apk add device-mapper
        dmsetup remove_all
    fi
    # Install arch have gpg-agent Process resident
    # exist aria2c Manually abort script while downloading，aria2c It will also download in the background
    killall -q gpg-agent aria2c || true
    umount_all

    # The following situations umount -R /1 Will prompt busy
    # mount /file1 /1
    # mount /1/file2 /2
}

# virt-what Automatic installation dmidecode，So cache both
cache_dmi_and_virt() {
    if ! [ "$_dmi_and_virt_cached" = 1 ]; then
        apk add virt-what

        # distinguish kvm and virtio，reason:
        # 1. Alibaba Cloud c8y virt-what Don’t show kvm
        # 2. not all kvm all require virtio drivers, for example aws nitro
        # 3. virt-what Will not detect virtio
        _virt=$(
            virt-what

            # hyper-v environment modprobe virtio_scsi will also create /sys/bus/virtio/drivers/virtio_scsi
            # Therefore use devices Judgment is more accurate only when equipment is available /sys/bus/virtio/drivers/*
            # Or add lspci Detection?

            # Don’t use ls /sys/bus/virtio/devices/* && echo virtio
            # Because it is possible that the return value is not 0 and interrupt script
            if ls /sys/bus/virtio/devices/* >/dev/null 2>&1; then
                echo virtio
            fi
        )

        _dmi=$(dmidecode | grep -E '(Manufacturer|Asset Tag|Vendor): ' | awk -F': ' '{print $2}')
        _dmi_and_virt_cached=1
        apk del virt-what
    fi
}

is_virt() {
    cache_dmi_and_virt
    [ -n "$_virt" ]
}

is_virt_contains() {
    cache_dmi_and_virt
    echo "$_virt" | grep -Eiwq "$1"
}

is_dmi_contains() {
    cache_dmi_and_virt
    echo "$_dmi" | grep -Eiwq "$1"
}

cache_lspci() {
    if [ -z "$_lspci" ]; then
        apk add pciutils
        _lspci=$(lspci)
        apk del pciutils
    fi
}

is_lspci_contains() {
    cache_lspci
    echo "$_lspci" | grep -Eiwq "$1"
}

get_config() {
    cat "/configs/$1"
}

set_config() {
    printf '%s' "$2" >"/configs/$1"
}

get_password_linux_sha512() {
    get_config password-linux-sha512
}

get_password_plaintext() {
    get_config password-plaintext
}

is_password_plaintext() {
    get_password_plaintext >/dev/null 2>&1
}

show_netconf() {
    grep -r . /dev/netconf/
}

get_ra_to() {
    if [ -z "$_ra" ]; then
        apk add ndisc6
        # Sometimes it will be collected repeatedly, so set it up to receive one copy and then exit.
        echo "Gathering network info..."
        # shellcheck disable=SC2154
        _ra="$(rdisc6 -1 "$ethx")"
        apk del ndisc6

        # Show network configuration
        info "Network info:"
        echo
        echo "$_ra" | cat -n
        echo
        ip addr | cat -n
        echo
        show_netconf | cat -n
        echo
    fi
    eval "$1='$_ra'"
}

get_netconf_to() {
    case "$1" in
    slaac | dhcpv6 | rdnss | other) get_ra_to ra ;;
    esac

    # shellcheck disable=SC2154
    case "$1" in
    slaac) echo "$ra" | grep 'Autonomous address conf' | grep -q Yes && res=1 || res=0 ;;
    dhcpv6) echo "$ra" | grep 'Stateful address conf' | grep -q Yes && res=1 || res=0 ;;
    rdnss) res=$(echo "$ra" | grep 'Recursive DNS server' | cut -d: -f2-) ;;
    other) echo "$ra" | grep 'Stateful other conf' | grep -q Yes && res=1 || res=0 ;;
    *) res=$(cat /dev/netconf/$ethx/$1) ;;
    esac

    eval "$1='$res'"
}

is_any_ipv4_has_internet() {
    grep -q 1 /dev/netconf/*/ipv4_has_internet
}

# have dhcpv4 It does not mean that there is a gateway, for example vultr pure ipv6
# No dhcpv4 Not equal to staticip，Maybe not ip
is_dhcpv4() {
    if ! is_ipv4_has_internet || should_disable_dhcpv4; then
        return 1
    fi

    get_netconf_to dhcpv4
    # shellcheck disable=SC2154
    [ "$dhcpv4" = 1 ]
}

is_staticv4() {
    if ! is_ipv4_has_internet; then
        return 1
    fi

    if ! is_dhcpv4; then
        get_netconf_to ipv4_addr
        get_netconf_to ipv4_gateway
        if [ -n "$ipv4_addr" ] && [ -n "$ipv4_gateway" ]; then
            return 0
        fi
    fi
    return 1
}

is_staticv6() {
    if ! is_ipv6_has_internet; then
        return 1
    fi

    if ! is_slaac && ! is_dhcpv6; then
        get_netconf_to ipv6_addr
        get_netconf_to ipv6_gateway
        if [ -n "$ipv6_addr" ] && [ -n "$ipv6_gateway" ]; then
            return 0
        fi
    fi
    return 1
}

is_dhcpv6_or_slaac() {
    get_netconf_to dhcpv6_or_slaac
    # shellcheck disable=SC2154
    [ "$dhcpv6_or_slaac" = 1 ]
}

is_ipv4_has_internet() {
    get_netconf_to ipv4_has_internet
    # shellcheck disable=SC2154
    [ "$ipv4_has_internet" = 1 ]
}

is_ipv6_has_internet() {
    get_netconf_to ipv6_has_internet
    # shellcheck disable=SC2154
    [ "$ipv6_has_internet" = 1 ]
}

should_disable_dhcpv4() {
    get_netconf_to should_disable_dhcpv4
    # shellcheck disable=SC2154
    [ "$should_disable_dhcpv4" = 1 ]
}

should_disable_accept_ra() {
    get_netconf_to should_disable_accept_ra
    # shellcheck disable=SC2154
    [ "$should_disable_accept_ra" = 1 ]
}

should_disable_autoconf() {
    get_netconf_to should_disable_autoconf
    # shellcheck disable=SC2154
    [ "$should_disable_autoconf" = 1 ]
}

is_slaac() {
    # If it is static (including automatically obtained IP But unable to connect to the Internet and switch to static) directly return 1，Don’t consider ra
    # handle machines thatslaac/dhcpv6obtainedip/Gateway cannot access the Internet

    # possible ra of dhcpv6/slaac It is open, but it cannot be obtained by actual measurement. ipv6 address
    # is_dhcpv6_or_slaac is the actual measurement result, so if the actual measurement fails, it will also be returned. 1

    # don’t judge is_staticv6，Because this will lead to an infinite loop
    if ! is_ipv6_has_internet || ! is_dhcpv6_or_slaac || should_disable_accept_ra || should_disable_autoconf; then
        return 1
    fi
    get_netconf_to slaac
    # shellcheck disable=SC2154
    [ "$slaac" = 1 ]
}

is_dhcpv6() {
    # If it is static (including automatically obtained IP But unable to connect to the Internet and switch to static) directly return 1，Don’t consider ra
    # handle machines thatslaac/dhcpv6obtainedip/Gateway cannot access the Internet

    # possible ra of dhcpv6/slaac It is open, but it cannot be obtained by actual measurement. ipv6 address
    # is_dhcpv6_or_slaac is the actual measurement result, so if the actual measurement fails, it will also be returned. 1

    # don’t judge is_staticv6，Because this will lead to an infinite loop
    if ! is_ipv6_has_internet || ! is_dhcpv6_or_slaac || should_disable_accept_ra || should_disable_autoconf; then
        return 1
    fi
    get_netconf_to dhcpv6

    # shellcheck disable=SC2154
    # Oracle even without adding IPv6 address，RA DHCPv6 The sign is also on
    # some systems must wait during startup DHCPv6 time out
    # This situation needs to be disabled DHCPv6
    if [ "$dhcpv6" = 1 ] && ! ip -6 -o addr show scope global dev "$ethx" | grep -q .; then
        echo 'DHCPv6 flag is on, but DHCPv6 is not working.'
        return 1
    fi

    [ "$dhcpv6" = 1 ]
}

is_have_ipv6() {
    is_slaac || is_dhcpv6 || is_staticv6
}

is_enable_other_flag() {
    get_netconf_to other
    # shellcheck disable=SC2154
    [ "$other" = 1 ]
}

is_have_rdnss() {
    # rdnss There may be several
    get_netconf_to rdnss
    [ -n "$rdnss" ]
}

# 15063 Or support it later rdnss

is_need_set_ssh_keys() {
    [ -s /configs/ssh_keys ]
}

is_need_change_ssh_port() {
    [ -n "$ssh_port" ] && ! [ "$ssh_port" = 22 ]
}

is_need_manual_set_dnsv6() {
    ! is_have_ipv6 && return $FALSE
    is_dhcpv6 && return $FALSE
    is_staticv6 && return $TRUE
    is_slaac && ! is_enable_other_flag && ! is_have_rdnss
}

get_current_dns() {
    mark=$(
        case "$1" in
        4) echo . ;;
        6) echo : ;;
        esac
    )
    grep '^nameserver' /etc/resolv.conf | cut -d' ' -f2 | grep -F "$mark" | cut -d '%' -f1
}

to_upper() {
    tr '[:lower:]' '[:upper:]'
}

to_lower() {
    tr '[:upper:]' '[:lower:]'
}

del_cr() {
    sed 's/\r$//'
}

del_comment_lines() {
    sed '/^[[:space:]]*#/d'
}

del_empty_lines() {
    sed '/^[[:space:]]*$/d'
}

del_head_empty_lines_inplace() {
    # from the first line until found ^[:space:]
    # remove all blank lines in this range
    sed -i '1,/[^[:space:]]/ { /^[[:space:]]*$/d }' "$@"
}

get_part_num_by_part() {
    dev_part=$1
    echo "$dev_part" | grep -o '[0-9]*' | tail -1
}

get_fallback_efi_file_name() {
    [ "$(uname -m)" = x86_64 ] || error_and_exit "EFI fallback is supported only on x86_64."
    echo bootx64.efi
}

del_invalid_efi_entry() {
    info "del invalid EFI entry"
    apk add lsblk efibootmgr

    efibootmgr --quiet --remove-dups

    while read -r line; do
        part_uuid=$(echo "$line" | awk -F ',' '{print $3}')
        efi_index=$(echo "$line" | grep_efi_index)
        if ! lsblk -o PARTUUID | grep -q "$part_uuid"; then
            echo "Delete invalid EFI Entry: $line"
            efibootmgr --quiet --bootnum "$efi_index" --delete-bootnum
        fi
    done < <(efibootmgr | grep 'HD(.*,GPT,')
}

# reinstall.sh There is a method with the same name
grep_efi_index() {
    awk '{print $1}' | sed -e 's/Boot//' -e 's/\*//'
}

# Some machines may not fall back to bootx64.efi
# Alibaba Cloud ECS The startup items are EFI Shell
# Add to bootx64.efi In the end, you will enter EFI Shell
# So add it to the front
add_default_efi_to_nvram() {
    info "add default EFI to nvram"

    apk add lsblk efibootmgr

    if efi_row=$(lsblk /dev/$xda -ro NAME,PARTTYPE,PARTUUID | grep -i "$EFI_UUID"); then
        efi_part_uuid=$(echo "$efi_row" | awk '{print $3}')
        efi_part_name=$(echo "$efi_row" | awk '{print $1}')
        efi_part_num=$(get_part_num_by_part "$efi_part_name")
        efi_file=$(get_fallback_efi_file_name)

        # Create an entry, first check whether it already exists
        # It seems that there is no need to judge first
        if true || ! efibootmgr | grep -i "HD($efi_part_num,GPT,$efi_part_uuid,.*)/File(\\\EFI\\\boot\\\\$efi_file)"; then
            efibootmgr --create \
                --disk "/dev/$xda" \
                --part "$efi_part_num" \
                --label "$efi_file" \
                --loader "\\EFI\\boot\\$efi_file"
        fi
    else
        # shellcheck disable=SC2154
        if [ "$confirmed_no_efi" = 1 ]; then
            echo 'Confirmed no EFI in previous step.'
        else
            # reinstall.sh already confirmed, but the logical sector size exceeds 512 when, it may be missed？
            # This should be judged based on logical sectors.？
            echo "
Warning: This machine is currently using EFI boot, but the main hard drive does not have an EFI partition.
If this machine supports Legacy BIOS boot (CSM), you can safely restart into the new system by running the reboot command.
If this machine does not support Legacy BIOS boot (CSM), you will not be able to enter the new system after rebooting.

Warning: This machine is currently using EFI Boots, but the primary hard drive does not EFI Partition。
If the machine supports Legacy BIOS guide (CSM)，You can run reboot Command to safely reboot to a new system。
If the machine does not support Legacy BIOS guide (CSM)，cannot boot the new system after reboot。
"
            exit
        fi
    fi
}

insert_into_file() {
    local file=$1
    local location=$2
    local regex_to_find=$3
    shift 3

    if ! [ -f "$file" ]; then
        error_and_exit "File not found: $file"
    fi

    # default grep -E
    if [ $# -eq 0 ]; then
        set -- -E
    fi

    if [ "$location" = head ]; then
        bak=$(mktemp)
        cp $file $bak
        cat - $bak >$file
    else
        line_num=$(grep "$@" -n "$regex_to_find" "$file" | cut -d: -f1)

        found_count=$(echo "$line_num" | wc -l)
        if [ ! "$found_count" -eq 1 ]; then
            return 1
        fi

        case "$location" in
        before) line_num=$((line_num - 1)) ;;
        replace)
            sed -i "${line_num}d" "$file"
            line_num=$((line_num - 1))
            ;;
        after) ;;
        *) return 1 ;;
        esac

        sed -i "${line_num}r /dev/stdin" "$file"
    fi
}

get_eths() {
    (
        cd /dev/netconf
        ls
    )
}

create_ifupdown_config() {
    conf_file=$1

    rm -f $conf_file

    # generate loconfiguration
    cat <<EOF >>$conf_file
auto lo
iface lo inet loopback
EOF

    # ethx
    for ethx in $(get_eths); do
        mode=auto
        # shellcheck disable=SC2154

        # head
        get_netconf_to mac_addr
        {
            echo
            # this is a marker，fix-eth-name Use it, don’t delete it
            # shellcheck disable=SC2154
            echo "# mac $mac_addr"
            echo $mode $ethx
        } >>$conf_file

        # ipv4
        if is_dhcpv4; then
            echo "iface $ethx inet dhcp" >>$conf_file

        elif is_staticv4; then
            get_netconf_to ipv4_addr
            get_netconf_to ipv4_gateway
            cat <<EOF >>$conf_file
iface $ethx inet static
    address $ipv4_addr
    gateway $ipv4_gateway
EOF
            # dns
            if list=$(get_current_dns 4); then
                for dns in $list; do
                    cat <<EOF >>$conf_file
    dns-nameservers $dns
EOF
                done
            fi
        fi

        # ipv6
        has_ipv6_iface=false
        if is_slaac; then
            echo "iface $ethx inet6 auto" >>$conf_file
            has_ipv6_iface=true
        elif is_dhcpv6; then
            echo "iface $ethx inet6 dhcp" >>$conf_file
            has_ipv6_iface=true
        elif is_staticv6; then
            get_netconf_to ipv6_addr
            get_netconf_to ipv6_gateway
            cat <<EOF >>$conf_file
iface $ethx inet6 static
    address $ipv6_addr
    gateway $ipv6_gateway
EOF
            has_ipv6_iface=true
            # additional IPv6 Address (the subnet does not include the address of the gateway）
            get_netconf_to ipv6_extra_addrs
            if [ -n "$ipv6_extra_addrs" ]; then
                (
                    IFS=','
                    for _addr in $ipv6_extra_addrs; do
                        echo "    post-up ip -6 addr add $_addr dev $ethx" >>$conf_file
                    done
                )
            fi
        fi
        # accept_ra/autoconf belong iface Options
        # If the current network card is not generated IPv6 iface stanza，
        # Make up one first manual stanza，avoid ifupdown newspaper misplaced option
        if ! $has_ipv6_iface &&
            { should_disable_accept_ra || should_disable_autoconf; }; then
            echo "iface $ethx inet6 manual" >>$conf_file
        fi
        # dns
        # have ipv6 But need to set dns situation
        if is_need_manual_set_dnsv6; then
            for dns in $(get_current_dns 6); do
                cat <<EOF >>$conf_file
    dns-nameserver $dns
EOF
            done
        fi

        # Disable ra
        if should_disable_accept_ra; then
            cat <<EOF >>$conf_file
    pre-up echo 0 >/proc/sys/net/ipv6/conf/$ethx/accept_ra
EOF
        fi

        # Disable autoconf
        if should_disable_autoconf; then
            cat <<EOF >>$conf_file
    pre-up echo 0 >/proc/sys/net/ipv6/conf/$ethx/autoconf
EOF
        fi
    done
}

newline_to_comma() {
    tr '\n' ','
}

space_to_newline() {
    sed 's/ /\n/g'
}

trim() {
    sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//'
}

quote_word() {
    sed -E 's/([^[:space:]]+)/"\1"/g'
}

quote_line() {
    awk '{print "\""$0"\""}'
}

add_space() {
    space_count=$1

    spaces=$(printf '%*s' "$space_count" '')
    sed "s/^/$spaces/"
}

# Not rigorous enough, use with caution

install_alpine() {
    info "install alpine"

    # alpine During installation, it will automatically detect what is needed for installation. firmware
    # https://github.com/alpinelinux/alpine-conf/blob/3.18.1/setup-disk.in#L421
    # but if not modloop cannot be detected
    # so delete modloop Record what is used before firmware Bag
    fw_pkgs=$(get_alpine_firmware_pkgs)

    # biosFor machines setup-disk Automatic partitioning will have boot Partition
    # So manual partition installation
    create_part
    mount_part_basic_layout /os /os/boot/efi

    # Network configuration
    create_ifupdown_config /etc/network/interfaces
    echo
    cat -n /etc/network/interfaces
    echo

    # The Alpine initramfs may have selected swclock before the target system was mounted.
    rc-update del swclock boot || true
    rc-update add hwclock boot

    # pass setup-alpine The installation will enable the following services
    # https://github.com/alpinelinux/alpine-conf/blob/3.18.1/setup-alpine.in#L229

    # boot
    rc-update add networking boot
    rc-update add seedrng boot

    # default
    rc-update add crond
    if [ -e /dev/input/event0 ]; then
        rc-update add acpid
    fi

    # Kernel
    # shellcheck disable=SC2154
    if [ "$no_cloud_kernel" = 1 ]; then
        kernel_flavor=lts
    else
        if is_virt; then
            kernel_flavor=virt
        else
            kernel_flavor=lts
        fi
    fi

    # reset to official repository configuration
    # Domestic aircraft may not be accessiblemirrorlist and report an error

    # setup-disk Install grub Skipped adding boot entry to nvram
    # handle machines that cannot fallback arrive bootx64.efi
    if is_efi; then
        apk add efibootmgr
        sed -i 's/--no-nvram//' "$(which setup-disk)"
    fi

    # Install to hard drive
    # alpineuse by default syslinux (efi Except environment)，It is mandatory to use here grub，Convenient to reinstall using scripts
    KERNELOPTS="$(get_ttys console=)"
    export KERNELOPTS
    export BOOTLOADER="grub"
    setup-disk -m sys -k $kernel_flavor /os

    # delete setup-disk Packages automatically installed when
    apk del e2fsprogs dosfstools efibootmgr grub*

    # If not mounted /proc

    # 1. chroot /os setup-keymap us us Will report an error
    # grep: /proc/filesystems: No such file or directory

    # 2. Installing firmware microcode will trigger grub-probe，If it is not mounted, an error will be reported.
    # Executing grub-2.12-r5.trigger
    # /usr/sbin/grub-probe: error: failed to get canonical path of `/dev/vda1'.
    # ERROR: grub-2.12-r5.trigger: script exited with error 1

    mount_pseudo_fs /os

    # azure nvme instance initramfs must add pci_hyperv driver
    if [ -d /sys/module/pci_hyperv ] &&
        get_drivers "/sys/block/$xda" | grep -qx pci_hyperv; then
        echo 'kernel/drivers/pci/controller/pci-hyperv.ko*' >/os/etc/mkinitfs/features.d/pci-hyperv.modules
        if ! grep -q 'pci-hyperv' /os/etc/mkinitfs/mkinitfs.conf; then
            # turn up features=" The line that starts with the last"Change to pci-hyperv"
            sed -i '/features="/s/"$/ pci-hyperv"/' /os/etc/mkinitfs/mkinitfs.conf
        fi
        chroot /os mkinitfs -k "$(basename /os/lib/modules/*-*)"
    fi

    # Install various applications after installing them to the hard disk.
    # avoid occupying Live OS Memory

    # network
    # udhcpc
    # pit1 ip -4 addr There is no way to know whether it is dhcp
    # pit2 networking Service will not run udhcpc6
    # pit3 h3c Mobile cloud computer udhcpc6 Unable to obtain dhcpv6

    # dhcpcd
    # pit1 slaacprivacy protection is enabled by default, causingipInconsistent with the background panel

    # slaacplan1: udhcpc + rdnssd
    # slaacplan2: dhcpcd + Turn off privacy protection
    # dhcpv6plan: dhcpcd

    # Comprehensive usedhcpcdplan
    # 1 No changes required/etc/network/interfaces，automatically based onrauseslaacanddhcpv6
    # 2 Bring your ownrdnsssupport
    # 3 The only thing to do is to turn off privacy protection

    # Install dhcpcd
    chroot /os apk add dhcpcd
    chroot /os sed -i '/^slaac private/s/^/#/' /etc/dhcpcd.conf
    chroot /os sed -i '/^#slaac hwaddr/s/^#//' /etc/dhcpcd.conf

    # Install other parts
    chroot /os setup-keymap us us
    chroot /os setup-timezone -i Asia/Shanghai
    # 3.21 default is chrony
    # 3.22 default is busybox ntp
    printf '\n' | chroot /os setup-ntp || true

    # Set public key
    add_user_if_need /os
    if is_need_set_ssh_keys; then
        set_ssh_keys_and_del_password /os
    fi

    # repair interface names on first boot
    download "$confhome/fix-eth-name.sh" /os/fix-eth-name.sh
    mkdir -p /os/etc/init.d
    write_fix_eth_name_openrc_service /os/etc/init.d/fix-eth-name
    chmod 755 /os/etc/init.d/fix-eth-name
    chroot /os rc-update add fix-eth-name boot

    # Install frpc
    if ls /configs/frpc.* >/dev/null 2>&1; then
        chroot /os apk add frp
        # chroot rc-update add add by default to sysinit
        # But don’t add chroot add by default to default
        chroot /os rc-update add frpc default

        # fixed to toml
        # This way you don’t have to /etc/init.d/frpc drop-in Revise cfgfile
        cp -f /configs/frpc.* /os/etc/frp/frpc.toml
        chmod 600 /os/etc/frp/frpc.toml
    fi

    # setup-disk Firmware is automatically selected, but microcode is not included？
    # https://github.com/alpinelinux/alpine-conf/blob/3.18.1/setup-disk.in#L421
    if fw_pkgs="$fw_pkgs $(get_ucode_firmware_pkgs)" && [ -n "$fw_pkgs" ]; then
        chroot /os apk add $fw_pkgs
    fi

    # 3.19 or above, not efi requires manual installation grub
    if ! is_efi; then
        chroot /os grub-install --target=i386-pc /dev/$xda
    fi

    # efi grub Add to fwsetup entry
    chroot /os update-grub

}

get_cpu_vendor() {
    cpu_vendor=$(grep 'vendor_id' /proc/cpuinfo | head -1 | awk '{print $NF}')
    case "$cpu_vendor" in
    GenuineIntel) echo intel ;;
    AuthenticAMD) echo amd ;;
    *) echo other ;;
    esac
}

min() {
    printf "%d\n" "$@" | sort -n | head -n 1
}

# Set up threads
# according to cpu Number of cores, memory per thread, take the minimum value
get_build_threads() {
    threads_per_mb=$1

    threads_by_core=$(nproc)
    threads_by_ram=$(($(get_approximate_ram_size) / threads_per_mb))
    [ $threads_by_ram -eq 0 ] && threads_by_ram=1
    min $threads_by_ram $threads_by_core
}

add_newline() {
    # shellcheck disable=SC1003
    case "$1" in
    head | start) sed -e '1s/^/\n/' ;;
    tail | end) sed -e '$a\\' ;;
    both) sed -e '1s/^/\n/' -e '$a\\' ;;
    esac
}

write_fix_eth_name_systemd_service() {
    cat >"$1" <<'EOF'
[Unit]
Description=Fix Eth Name
ConditionPathExists=/fix-eth-name.sh

After=dbus.service

Before=networking.service
Before=systemd-networkd.service

Before=network.target

[Service]
Type=oneshot

StandardOutput=journal+console
StandardError=journal+console

ExecStart=/usr/bin/env bash /fix-eth-name.sh
ExecStart=/usr/bin/env rm -f /fix-eth-name.sh
ExecStart=/usr/bin/env rm -f /etc/systemd/system/fix-eth-name.service
ExecStart=/usr/bin/env rm -f /etc/systemd/system/multi-user.target.wants/fix-eth-name.service
ExecStart=/usr/bin/env rm -f /lib/systemd/system-preset/01-fix-eth-name.preset
ExecStart=/usr/bin/env rm -f /usr/lib/systemd/system-preset/01-fix-eth-name.preset

[Install]
WantedBy=multi-user.target
EOF
}

write_fix_eth_name_openrc_service() {
    cat >"$1" <<'EOF'
#!/sbin/openrc-run
# shellcheck shell=sh

# shellcheck disable=SC2034 # OpenRC reads this variable.
Description="Fix Eth Name"

# https://gitlab.alpinelinux.org/alpine/aports/-/blob/master/main/openrc/networking.initd
# https://gitlab.alpinelinux.org/alpine/aports/-/blob/master/main/dhcpcd/dhcpcd.initd
depend() {
    need localmount
    want dev-settle

    after bootmisc hwdrivers modules
    before net networking dhcpcd
}

start() {
    ebegin "Fix Eth Name"
    ash /fix-eth-name.sh
    eend $?
}

start_post() {
    rc-service fix-eth-name zap
    rc-update del fix-eth-name boot
    rm -f /etc/init.d/fix-eth-name
    rm -f /fix-eth-name.sh
}
EOF
}

write_frpc_systemd_service() {
    cat >"$1" <<'EOF'
# https://github.com/archlinuxcn/repo/blob/master/archlinuxcn/frp/frpc.service

[Unit]
Description=Frp Client Service
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
Restart=on-failure
RestartSec=5s
DynamicUser=yes
LoadCredential=frpc.toml:/usr/local/etc/frpc/frpc.toml
ExecStart=/usr/local/bin/frpc -c ${CREDENTIALS_DIRECTORY}/frpc.toml
ExecReload=/usr/local/bin/frpc reload -c ${CREDENTIALS_DIRECTORY}/frpc.toml

[Install]
WantedBy=multi-user.target
EOF
}

add_systemd_service() {
    local os_dir=$1
    local service_name=$2
    local service_path="$os_dir/etc/systemd/system/$service_name.service"

    mkdir -p "$os_dir/etc/systemd/system"
    case "$service_name" in
    fix-eth-name) write_fix_eth_name_systemd_service "$service_path" ;;
    frpc) write_frpc_systemd_service "$service_path" ;;
    *) error_and_exit "Unknown generated systemd service: $service_name" ;;
    esac
    chmod 644 "$service_path"
    chroot "$os_dir" systemctl enable "$service_name.service"

    # Keep the generated one-shot/network services enabled across target presets.

    # may be /usr/lib/systemd/system-preset/ or /lib/systemd/system-preset/
    if [ -d "$os_dir/usr/lib/systemd/system-preset" ]; then
        echo "enable $service_name.service" >"$os_dir/usr/lib/systemd/system-preset/01-$service_name.preset"
    else
        echo "enable $service_name.service" >"$os_dir/lib/systemd/system-preset/01-$service_name.preset"
    fi
}

add_fix_eth_name_systemd_service() {
    local os_dir=$1

    # No need to execute systemctl daemon-reload
    # because chroot The next execution will prompt Running in chroot, ignoring command 'daemon-reload'
    download "$confhome/fix-eth-name.sh" "$os_dir/fix-eth-name.sh"
    add_systemd_service "$os_dir" fix-eth-name
}

is_ipv6_only() {
    ! grep -q 1 /dev/netconf/*/ipv4_has_internet
}

# Keep the standalone helper's external wget behavior; do not use trans.sh's retry wrapper.
frpc_wget() {
    command wget "$@"
}

get_frpc_url() {
    [ "${1:-linux}" = linux ] && [ "$#" -le 1 ] || {
        echo 'Only the Linux frpc target is supported.' >&2
        return 1
    }
    [ "$(uname -m)" = x86_64 ] || {
        echo 'Only x86_64 frpc binaries are supported.' >&2
        return 1
    }
    local version mirror
    # GitHub releases are IPv4-only. Preserve the original IPv6 connectivity
    # fallback independently of geography; this is NOT a China-location check.
    if is_ipv6_only; then
        mirror=https://mirrors.nju.edu.cn/github-release/fatedier/frp
        version=$(frpc_wget -O- "$mirror/LatestRelease/frp_sha256_checksums.txt" |
            grep -m1 frp_ | cut -d_ -f2)
    else
        mirror=https://github.com/fatedier/frp/releases/download
        version=$(frpc_wget --spider -S https://github.com/fatedier/frp/releases/latest 2>&1 |
            grep -m1 '^  Location:' | sed 's,.*/tag/v,,')
    fi
    echo "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || {
        echo 'Cannot resolve the latest frpc version.' >&2
        return 1
    }
    echo "$mirror/v$version/frp_${version}_linux_amd64.tar.gz"
}

add_frpc_systemd_service_if_need() {
    local os_dir=$1

    if ls /configs/frpc.* >/dev/null 2>&1; then
        mkdir -p "$os_dir/usr/local/bin"
        mkdir -p "$os_dir/usr/local/etc/frpc"

        # download frpc
        # Pay attention to downloading frpc owner no root:root
        frpc_url=$(get_frpc_url linux)
        basename=$(echo "$frpc_url" | awk -F/ '{print $NF}' | sed 's/\.tar\.gz//')
        download "$frpc_url" "$os_dir/frpc.tar.gz"
        # busybox tar Not supported wildcard
        # tar: */frpc: not found in archive
        tar xzf "$os_dir/frpc.tar.gz" "$basename/frpc" -O >"$os_dir/usr/local/bin/frpc"
        rm -f "$os_dir/frpc.tar.gz"
        chmod a+x "$os_dir/usr/local/bin/frpc"

        # frpc toml
        cp -f /configs/frpc.* "$os_dir/usr/local/etc/frpc/frpc.toml"
        # Retained systemd targets use LoadCredential (systemd >= 247).
        chroot "$os_dir" chown root:root /usr/local/etc/frpc/frpc.toml
        chroot "$os_dir" chmod 600 /usr/local/etc/frpc/frpc.toml

        # Add service
        add_systemd_service "$os_dir" frpc
    fi
}

# Shared Linux hardware helper: Alpine's Azure NVMe initramfs needs this too.
get_drivers() {
    (
        cd "$(readlink -f $1)"
        while ! [ "$(pwd)" = / ]; do
            if [ -d driver ]; then
                if [ -d driver/module ]; then
                    basename "$(readlink -f driver/module)"
                else
                    basename "$(readlink -f driver)"
                fi
            fi
            cd ..
        done
    )
}

get_fs_of_mount_point() {
    local mount_point=$1

    if ! [ "$mount_point" = / ]; then
        # Delete the last few /
        mount_point=$(printf "%s" "$mount_point" | sed 's,/*$,,')
    fi

    # findmnt To install
    # findmnt "$mount_point" -rno FSTYPE
    mount | awk -v mp="$1" '$3==mp {print $5}' | grep .
}

basic_init() {
    local os_dir=$1

    # Not available at this time
    # chroot $os_dir timedatectl set-timezone Asia/Shanghai
    # Failed to create bus connection: No such file or directory

    if is_have_cmd_on_disk $os_dir systemd-firstboot; then
        if chroot $os_dir systemd-firstboot --help | grep -wq '\--force'; then
            chroot $os_dir systemd-firstboot --timezone=Asia/Shanghai --force
        else
            chroot $os_dir systemd-firstboot --timezone=Asia/Shanghai
        fi
    fi

    # gentoo will not be created automatically machine-id
    clear_machine_id $os_dir

    # sshd
    chroot $os_dir ssh-keygen -A

    sshd_enabled=false
    sshs="sshd.service ssh.service sshd.socket ssh.socket"
    for i in $sshs; do
        if chroot $os_dir systemctl -q is-enabled $i; then
            sshd_enabled=true
            break
        fi
    done
    if ! $sshd_enabled; then
        for i in $sshs; do
            if chroot $os_dir systemctl -q enable $i; then
                break
            fi
        done
    fi

    if is_need_change_ssh_port; then
        change_ssh_port $os_dir $ssh_port
    fi

    # public key/password
    add_user_if_need "$os_dir"
    if is_need_set_ssh_keys; then
        set_ssh_keys_and_del_password $os_dir
        change_ssh_conf_for_key_login $os_dir
    else
        change_user_password $os_dir
        change_ssh_conf_for_password_login $os_dir
    fi

    # generate fix-eth-name.service
    # Even if it is open net.ifnames=0 Also needed
    # because alpine live The order of network cards may be different from that of the target system.
    add_fix_eth_name_systemd_service $os_dir

    # frpc
    add_frpc_systemd_service_if_need $os_dir
}

setup_systemd_networkd() {
    local os_dir=$1 work_dir=${2:-/tmp/reinstall-networkd-config.$$}

    chroot "$os_dir" systemctl enable systemd-networkd.service
    chroot "$os_dir" systemctl enable systemd-resolved.service

    apk add cloud-init
    useradd systemd-network || true
    mkdir_clear "$work_dir"
    create_cloud_init_network_config "$work_dir/net.cfg"
    cloud-init devel net-convert -p "$work_dir/net.cfg" -k yaml -d "$work_dir/out" -D alpine -O networkd

    mkdir -p "$os_dir/etc/systemd/network"
    cp "$work_dir/out"/etc/systemd/network/10-cloud-init-eth*.network \
        "$os_dir/etc/systemd/network/"
    sed -i '/^Name=/d' "$os_dir"/etc/systemd/network/10-cloud-init-eth*.network
    sed -i '/^# Generated by cloud-init/d' "$os_dir"/etc/systemd/network/10-cloud-init-eth*.network
    del_head_empty_lines_inplace "$os_dir"/etc/systemd/network/10-cloud-init-eth*.network

    cat -n "$os_dir"/etc/systemd/network/10-cloud-init-eth*.network
    rm -rf "$work_dir"
    apk del cloud-init
}

debian_kernel_package() {
    if [ "$no_cloud_kernel" = 1 ]; then
        echo linux-image-amd64
    else
        echo linux-image-cloud-amd64
    fi
}

bootstrap_debian_rootfs() {
    local os_dir=$1 archive=${mirror:-https://deb.debian.org/debian}
    [[ "$codename" =~ ^[a-z]+$ ]] || error_and_exit "Invalid Debian stable codename."
    [[ "$archive" =~ ^https://[A-Za-z0-9._~:/-]+$ ]] || error_and_exit "Invalid Debian archive URL."
    apk add debootstrap
    debootstrap --arch=amd64 --variant=minbase --include=ca-certificates,debconf-utils \
        "$codename" "$os_dir" "$archive"
}

write_debian_apt_sources() {
    local os_dir=$1 archive=${mirror:-https://deb.debian.org/debian} sources sources_tmp
    [[ "$codename" =~ ^[a-z]+$ ]] || error_and_exit "Invalid Debian stable codename."
    [[ "$archive" =~ ^https://[A-Za-z0-9._~:/-]+$ ]] || error_and_exit "Invalid Debian archive URL."
    mkdir -p "$os_dir/etc/apt/sources.list.d"
    sources=$os_dir/etc/apt/sources.list
    sources_tmp=$sources.tmp.$$
    cat >"$sources_tmp" <<EOF
# Debian stable repositories
deb $archive $codename main contrib non-free non-free-firmware
deb $archive $codename-updates main contrib non-free non-free-firmware
deb https://security.debian.org/debian-security $codename-security main contrib non-free non-free-firmware
EOF
    mv -f "$sources_tmp" "$sources"
    rm -f "$os_dir/etc/apt/sources.list.d/debian.sources"
}

write_debian_fstab() {
    local os_dir=$1 root_part root_uuid esp_uuid efi=false fstab fstab_tmp
    is_efi && efi=true
    if $efi || is_xda_gt_2t; then
        root_part=$(xda 2)
    else
        root_part=$(xda 1)
    fi
    root_uuid=$(blkid -s UUID -o value "/dev/$root_part") ||
        error_and_exit "Cannot read Debian root filesystem UUID."
    [[ "$root_uuid" =~ ^[A-Fa-f0-9-]+$ ]] || error_and_exit "Invalid Debian root filesystem UUID."
    if $efi; then
        esp_uuid=$(blkid -s UUID -o value "/dev/$(xda 1)") ||
            error_and_exit "Cannot read EFI system partition UUID."
        [[ "$esp_uuid" =~ ^[A-Fa-f0-9-]+$ ]] || error_and_exit "Invalid EFI system partition UUID."
    fi

    mkdir -p "$os_dir/etc"
    fstab=$os_dir/etc/fstab
    fstab_tmp=$fstab.tmp.$$
    {
        printf '%s\n' '# <file system> <mount point> <type> <options> <dump> <pass>'
        printf 'UUID=%s / ext4 defaults,errors=remount-ro 0 1\n' "$root_uuid"
        if $efi; then
            printf 'UUID=%s /boot/efi vfat umask=0077 0 1\n' "$esp_uuid"
        fi
    } >"$fstab_tmp"
    mv -f "$fstab_tmp" "$fstab"
}

prepare_debian_grub_config() {
    local os_dir=$1
    if is_efi; then
        printf '%s\n' \
            'grub-efi-amd64 grub-efi/install_devices_empty boolean true' \
            'grub-efi-amd64 grub-efi/install_devices multiselect' |
            chroot "$os_dir" debconf-set-selections
    else
        printf 'grub-pc grub-pc/install_devices multiselect /dev/%s\n' "$xda" |
            chroot "$os_dir" debconf-set-selections
    fi
}

install_debian_grub() {
    local os_dir=$1 ttys_cmdline
    if is_efi; then
        chroot "$os_dir" grub-install --target=x86_64-efi --efi-directory=/boot/efi \
            --bootloader-id=debian --no-nvram --removable
    else
        chroot "$os_dir" grub-install --target=i386-pc "/dev/$xda"
    fi
    mkdir -p "$os_dir/etc/default"
    ttys_cmdline=$(get_ttys console=)
    printf 'GRUB_CMDLINE_LINUX="$GRUB_CMDLINE_LINUX %s"\\n' "$ttys_cmdline" \
        >>"$os_dir/etc/default/grub"
    chroot "$os_dir" grub-mkconfig -o /boot/grub/grub.cfg
}

install_debian() {
    local os_dir=${1:-/os} kernel_pkg grub_pkg packages firmware_pkgs
    info "install Debian stable via Alpine debootstrap"
    mount_part_basic_layout "$os_dir" "$os_dir/boot/efi"
    bootstrap_debian_rootfs "$os_dir"
    cp_resolv_conf "$os_dir"
    mount_pseudo_fs "$os_dir"
    write_debian_apt_sources "$os_dir"
    write_debian_fstab "$os_dir"

    kernel_pkg=$(debian_kernel_package)
    if is_efi; then
        grub_pkg=grub-efi-amd64
    else
        grub_pkg=grub-pc
    fi
    prepare_debian_grub_config "$os_dir"
    packages="ca-certificates systemd-sysv systemd-resolved openssh-server $kernel_pkg $grub_pkg"
    if is_efi; then packages="$packages efibootmgr dosfstools"; fi
    if [ "$username" != root ]; then packages="$packages sudo"; fi
    if firmware_pkgs=$(get_ucode_firmware_pkgs) && [ -n "$firmware_pkgs" ]; then
        packages="$packages $firmware_pkgs"
    fi

    DEBIAN_FRONTEND=noninteractive chroot "$os_dir" apt-get update
    DEBIAN_FRONTEND=noninteractive chroot "$os_dir" apt-get install -y --no-install-recommends $packages

    setup_systemd_networkd "$os_dir"
    rm_resolv_conf "$os_dir"
    ln -s /run/systemd/resolve/stub-resolv.conf "$os_dir/etc/resolv.conf"
    basic_init "$os_dir"
    install_debian_grub "$os_dir"
}

install_arch_gentoo() {
    info "install $distro"

    set_locale() {
        echo "C.UTF-8 UTF-8" >>$os_dir/etc/locale.gen
        chroot $os_dir locale-gen
    }

    # shellcheck disable=SC2317
    install_arch() {
        local alpine_rootfs=$os_dir/alpine
        create_alpine_rootfs_with_arch_install_scripts "$alpine_rootfs" true "$os_dir"

        # For the second runtime /etc/pacman.conf Unmodified
        if [ -f $alpine_rootfs/etc/pacman.conf.orig ]; then
            cp $alpine_rootfs/etc/pacman.conf.orig $alpine_rootfs/etc/pacman.conf
        else
            cp $alpine_rootfs/etc/pacman.conf $alpine_rootfs/etc/pacman.conf.orig
        fi

        # set up repo
        insert_into_file $alpine_rootfs/etc/pacman.conf before '\[core\]' <<EOF
SigLevel = Never
ParallelDownloads = 5
EOF
        cat <<EOF >>$alpine_rootfs/etc/pacman.conf
[core]
Include = /etc/pacman.d/mirrorlist

[extra]
Include = /etc/pacman.d/mirrorlist
EOF
        mkdir -p $alpine_rootfs/etc/pacman.d
        # shellcheck disable=SC2016,SC2154
        dir='$repo/os/$arch'
        echo "Server = $mirror/$dir" >$alpine_rootfs/etc/pacman.d/mirrorlist

        # Install system
        # To install partitioning tools(Include fsck.xxx)，used for initramfs Check partition data
        pkgs="base grub openssh"

        # efi fs
        if is_efi; then
            pkgs="$pkgs efibootmgr dosfstools"
        fi

        # root fs
        case $(get_fs_of_mount_point "$os_dir") in
        xfs) pkgs="$pkgs xfsprogs" ;;
        ext4) pkgs="$pkgs e2fsprogs" ;;
        btrfs) pkgs="$pkgs btrfs-progs" ;;
        esac

        if ! [ "$username" = root ]; then
            pkgs="$pkgs sudo"
        fi

        # retry prevent network failures
        if [ "$alpine_rootfs" = / ]; then
            retry 5 pacstrap -K "$os_dir" $pkgs
            killall -q gpg-agent || true
            apk del arch-install-scripts
        else
            retry 5 chroot "$alpine_rootfs" pacstrap -K "/parent" $pkgs
            killall -q gpg-agent || true
            umount -R "$alpine_rootfs/parent"
            remove_alpine_rootfs "$alpine_rootfs"
        fi

        # dns
        cp_resolv_conf $os_dir

        # Mount pseudo file system
        mount_pseudo_fs $os_dir

        # You need to set the language first and then install the kernel, otherwise it will appear
        # ==> Creating gzip-compressed initcpio image: '/boot/initramfs-linux.img'
        # bsdtar: bsdtar: Failed to set default locale
        # Failed to set default locale
        set_locale

        # firmware + microcode
        if fw_pkgs=$(get_ucode_firmware_pkgs) && [ -n "$fw_pkgs" ]; then
            chroot $os_dir pacman -Syu --noconfirm $fw_pkgs
        fi

        chroot $os_dir pacman -Syu --noconfirm linux
    }

    # shellcheck disable=SC2317
    install_gentoo() {
        # Decompression system
        apk add tar xz pv
        # shellcheck disable=SC2154
        download "$img" $os_dir/gentoo.tar.xz
        echo "Uncompressing Gentoo..."
        pv -f $os_dir/gentoo.tar.xz | tar xpJ --numeric-owner --xattrs-include='*.*' -C $os_dir
        rm $os_dir/gentoo.tar.xz
        apk del tar xz pv

        # dns
        cp_resolv_conf $os_dir

        # Mount pseudo file system
        mount_pseudo_fs $os_dir

        # Download the repository and select profile
        chroot $os_dir emerge-webrsync
        profile=$(
            # filter stable systemd，Then choose the shortest
            chroot $os_dir eselect profile list | grep stable | grep systemd |
                awk '{print length($2), $2}' | sort -n | head -1 | awk '{print $2}'
        )
        echo "Select profile: $profile"
        chroot $os_dir eselect profile set $profile

        # set up license
        cat <<EOF >>$os_dir/etc/portage/make.conf
ACCEPT_LICENSE="*"
EOF

        cat <<EOF >>$os_dir/etc/portage/make.conf
MAKEOPTS="-j$(get_build_threads 2048)"
EOF

        # set up http repo + binpkg repo
        mirror_short=$(echo "$img" | sed 's,/releases/.*,,')
        mirror_long=$(echo "$img" | sed 's,/autobuilds/.*,,')
        profile_ver=$(chroot $os_dir eselect profile show | grep -Eo '/[0-9.]*/' | cut -d/ -f2)

        if chroot $os_dir ld.so --help | grep supported | grep -q x86-64-v3; then
            binpkg_type=x86-64-v3
        else
            binpkg_type=x86-64
        fi

        cat <<EOF >>$os_dir/etc/portage/make.conf
GENTOO_MIRRORS="$mirror_short"
FEATURES="getbinpkg"
EOF

        cat <<EOF >$os_dir/etc/portage/binrepos.conf/gentoobinhost.conf
[binhost]
priority = 9999
sync-uri = $mirror_long/binpackages/$profile_ver/$binpkg_type
EOF

        # Download public key

        # getuto will determine whether there is ${TERM} and ${TERM} no dumb
        # If conditions are met source /lib/gentoo/functions.sh import ebegin etc.
        # If the conditions are not met, create it yourself ebegin etc.

        # /lib/gentoo/functions.sh will determine whether there is ${RC_OPENRC_PID}
        # If there is, it will not be imported. /functions/openrc.sh，It will not be imported ebegin method

        # exist ssh run within /trans.sh when, no ${RC_OPENRC_PID}，${TERM} yes xterm
        # therefore chroot $os_dir getuto No error will be reported when

        # exist locald run within /trans.sh sometimes, there is ${RC_OPENRC_PID}，${TERM} yes linux
        # therefore chroot $os_dir getuto Sometimes an error will be reported saying ebegin: command not found

        # There are two solutions
        if true; then
            TERM=dumb chroot $os_dir getuto
        else
            env -u RC_OPENRC_PID chroot $os_dir getuto
        fi

        set_locale

        # Install git Will be upgraded glibc，at this time /etc/locale.gen It cannot be empty, otherwise it will prompt to generate all locale
        # Generating all locales; edit /etc/locale.gen to save time/space
        chroot $os_dir emerge dev-vcs/git

        # set up git repo
        # github Not supported ipv6
        is_any_ipv4_has_internet && git_uri=https://github.com/gentoo-mirror/gentoo.git ||
            git_uri=https://anongit.gentoo.org/git/repo/gentoo.git

        mkdir -p $os_dir/etc/portage/repos.conf
        cat <<EOF >$os_dir/etc/portage/repos.conf/gentoo.conf
[gentoo]
location = /var/db/repos/gentoo
sync-type = git
sync-uri = $git_uri
EOF
        rm -rf $os_dir/var/db/repos/gentoo
        chroot $os_dir emerge --sync

        # Install all packages at once to reduce package duplication rebuild ?
        local pkgs=

        # https://wiki.gentoo.org/wiki/Handbook:AMD64/Installation/Tools#Filesystem_tools
        pkgs="$pkgs sys-block/io-scheduler-udev-rules"

        # efi fs
        if is_efi; then
            pkgs="$pkgs sys-fs/dosfstools sys-boot/efibootmgr"
        fi

        # root fs
        case $(get_fs_of_mount_point "$os_dir") in
        xfs) pkgs="$pkgs sys-fs/xfsprogs" ;;
        ext4) pkgs="$pkgs sys-fs/e2fsprogs" ;;
        btrfs) pkgs="$pkgs sys-fs/btrfs-progs" ;;
        esac

        # sudo
        if ! [ "$username" = root ]; then
            pkgs="$pkgs app-admin/sudo"
        fi

        # firmware + microcode
        if fw_pkgs=$(get_ucode_firmware_pkgs) && [ -n "$fw_pkgs" ]; then
            pkgs="$pkgs $fw_pkgs"
        fi

        # Install grub + Kernel
        is_efi && grub_platforms="efi-64" || grub_platforms="pc"
        echo GRUB_PLATFORMS=\"$grub_platforms\" >>$os_dir/etc/portage/make.conf
        echo "sys-kernel/installkernel dracut grub" >$os_dir/etc/portage/package.use/installkernel

        # To set root=UUID=xxxx，otherwise dracut Will report an error
        # Pay attention to root=UUID=xxxx There are spaces at the beginning and end
        # https://wiki.gentoo.org/wiki/Installkernel#Install_chroot_check
        # https://wiki.gentoo.org/wiki/Handbook:AMD64/Installation/Kernel#Chroot_detection
        uuid=$(chroot $os_dir findmnt -rno UUID /)
        mkdir -p $os_dir/etc/dracut.conf.d
        echo "kernel_cmdline=\" root=UUID=$uuid \"" >$os_dir/etc/dracut.conf.d/00-installkernel.conf
        pkgs="$pkgs sys-kernel/gentoo-kernel-bin"

        # Install
        # Do not add -n/--noreplace If it would rebuild Selected packages, whether installed or not
        chroot "$os_dir" emerge -n $pkgs
    }

    local os_dir=/os

    # Mount partition
    mount_part_basic_layout /os /os/efi

    # Install system
    install_$distro

    # Install arch have gpg-agent Process resident
    killall -q gpg-agent || true

    # initialization

    # Network configuration is shared by Arch, Gentoo and Debian.
    setup_systemd_networkd "$os_dir"

    basic_init $os_dir

    # ntp use systemd Comes with it
    # TODO: vm agent + random number generator

    # grub
    if is_efi; then
        # arch gentoo recommend efi mounted on /efi
        chroot $os_dir grub-install --efi-directory=/efi
        chroot $os_dir grub-install --efi-directory=/efi --removable
    else
        chroot $os_dir grub-install /dev/$xda
    fi

    # cmdline + generate grub.cfg
    if [ -d $os_dir/etc/default/grub.d ]; then
        file=$os_dir/etc/default/grub.d/tty.cfg
    else
        file=$os_dir/etc/default/grub
    fi
    ttys_cmdline=$(get_ttys console=)
    echo GRUB_CMDLINE_LINUX=\"\$GRUB_CMDLINE_LINUX $ttys_cmdline\" >>$file
    chroot $os_dir grub-mkconfig -o /boot/grub/grub.cfg

    # fstab
    # fstab Don’t write it efi entry， systemd automount will be automatically mounted
    # fstab There are instructions for use on the head, so use >>
    local alpine_rootfs=$os_dir/alpine
    create_alpine_rootfs_with_arch_install_scripts "$alpine_rootfs" true "$os_dir"
    # genfstab will be used findmnt and other tools
    retry 5 chroot "$alpine_rootfs" apk add util-linux
    chroot "$alpine_rootfs" genfstab -U /parent | sed '/swap/d' >>$os_dir/etc/fstab
    umount -R "$alpine_rootfs/parent"
    remove_alpine_rootfs "$alpine_rootfs"

    # delete resolv.conf，otherwise systemd-resolved Unable to create soft link
    rm_resolv_conf $os_dir

}

aria2c() {
    if ! is_have_cmd aria2c; then
        apk add aria2
    fi

    # stdbuf exist coreutils Inside the bag
    if ! is_have_cmd stdbuf; then
        apk add coreutils
    fi

    # show url
    show_url_in_args "$@" >&2

    retry 5 5 stdbuf -oL -eL aria2c \
        -x4 \
        --allow-overwrite=true \
        --summary-interval=0 \
        --max-tries 1 \
        "$@"
}

get_disk_sector_count() {
    # cat /proc/partitions
    blockdev --getsz "$1"
}

get_disk_size() {
    blockdev --getsize64 "$1"
}

get_disk_logic_sector_size() {
    blockdev --getss "$1"
}

is_4kn() {
    [ "$(blockdev --getss "/dev/$xda")" = 4096 ]
}

is_xda_gt_2t() {
    disk_size=$(get_disk_size /dev/$xda)
    disk_2t=$((2 * 1024 * 1024 * 1024 * 1024))
    [ "$disk_size" -gt "$disk_2t" ]
}

is_ends_with_digit() {
    [[ "$1" =~ [0-9]$ ]]
}

xda() {
    if [ -n "$1" ]; then
        if is_ends_with_digit "$xda"; then
            echo "${xda}p$1"
        else
            echo "${xda}$1"
        fi
    else
        echo "$xda"
    fi
}

create_part() {
    info "Create Part"
    apk add parted e2fsprogs
    if is_efi; then apk add dosfstools; fi
    apk add wipefs
    wipefs -a -f /dev/$xda
    apk del wipefs
    # alpine itself closed 64bit ext4
    # https://gitlab.alpinelinux.org/alpine/alpine-conf/-/blob/3.18.1/setup-disk.in?ref_type=tags#L908
    # and alpine of extlinux Not compatible 64bit ext4
    [ "$distro" = alpine ] && ext4_opts="-O ^64bit" || ext4_opts=
    if is_efi; then
        # efi
        parted /dev/$xda -s -- \
            mklabel gpt \
            mkpart '" "' fat32 1MiB 101MiB \
            mkpart '" "' ext4 101MiB 100% \
            set 1 boot on
        update_part

        mkfs.fat "/dev/$(xda 1)"                #1 efi
        mkfs.ext4 -F $ext4_opts "/dev/$(xda 2)" #2 os
    elif is_xda_gt_2t; then
        # bios > 2t
        parted /dev/$xda -s -- \
            mklabel gpt \
            mkpart '" "' ext4 1MiB 2MiB \
            mkpart '" "' ext4 2MiB 100% \
            set 1 bios_grub on
        update_part

        echo                                    #1 bios_boot
        mkfs.ext4 -F $ext4_opts "/dev/$(xda 2)" #2 os
    else
        # bios
        parted /dev/$xda -s -- \
            mklabel msdos \
            mkpart primary ext4 1MiB 100% \
            set 1 boot on
        update_part

        mkfs.ext4 -F $ext4_opts "/dev/$(xda 1)" #1 os
    fi

    update_part
    if [ "$distro" = alpine ]; then apk del parted; fi
}

umount_pseudo_fs() {
    local os_dir
    os_dir=$(realpath "$1")

    dirs="/proc /sys /dev /run"
    regex=$(echo "$dirs" | sed 's, ,|,g')
    if mounts=$(mount | grep -Ew "on $os_dir($regex)" | awk '{print $3}' | tac); then
        for mount in $mounts; do
            echo "umount $mount"
            umount $mount
        done
    fi
}

mount_pseudo_fs() {
    local os_dir=$1

    mkdir -p $os_dir/proc/ $os_dir/sys/ $os_dir/dev/ $os_dir/run/

    # https://wiki.archlinux.org/title/Chroot#Using_chroot
    mount -t proc /proc $os_dir/proc/
    mount -t sysfs /sys $os_dir/sys/
    mount --rbind /dev $os_dir/dev/
    mount --rbind /run $os_dir/run/
    if is_efi; then
        mount --rbind /sys/firmware/efi/efivars $os_dir/sys/firmware/efi/efivars/
    fi
}

create_cloud_init_network_config() {
    ci_file=$1
    recognize_static6=${2:-true}
    recognize_ipv6_types=${3:-true}

    info "Create Cloud-init network config"

    # prevent a missing file
    mkdir -p "$(dirname "$ci_file")"
    touch "$ci_file"

    apk add yq-go

    need_set_dns4=false
    need_set_dns6=false

    config_id=0
    for ethx in $(get_eths); do
        get_netconf_to mac_addr

        # shellcheck disable=SC2154
        yq -i ".network.version=1 |
           .network.config[$config_id].type=\"physical\" |
           .network.config[$config_id].name=\"$ethx\" |
           .network.config[$config_id].mac_address=(\"$mac_addr\" | . style=\"single\")
           " $ci_file

        subnet_id=0

        # ipv4
        if is_dhcpv4; then
            yq -i ".network.config[$config_id].subnets[$subnet_id] = {\"type\": \"dhcp4\"}" $ci_file
            subnet_id=$((subnet_id + 1))
        elif is_staticv4; then
            need_set_dns4=true
            get_netconf_to ipv4_addr
            get_netconf_to ipv4_gateway
            yq -i ".network.config[$config_id].subnets[$subnet_id] = {
                    \"type\": \"static\",
                    \"address\": \"$ipv4_addr\",
                    \"gateway\": \"$ipv4_gateway\" }
                    " $ci_file

            # Old version cloud-init have bug
            # Some versions will only read from the first configuration dns，Some read from the second type
            # So write two configurations
            # https://github.com/canonical/cloud-init/commit/1b8030e0c7fd6fbff7e38ad1e3e6266ae50c83a5
            for cur in $(get_current_dns 4); do
                yq -i ".network.config[$config_id].subnets[$subnet_id].dns_nameservers += [\"$cur\"]" $ci_file
            done
            subnet_id=$((subnet_id + 1))
        fi

        # ipv6
        # slaac:  ipv6_slaac
        # └─enable_other_flag: ipv6_dhcpv6-stateless
        # dhcpv6: ipv6_dhcpv6-stateful

        # ipv6
        if is_slaac; then
            if $recognize_ipv6_types; then
                if is_enable_other_flag; then
                    type=ipv6_dhcpv6-stateless
                else
                    type=ipv6_slaac
                fi
            else
                type=dhcp6
            fi
            yq -i ".network.config[$config_id].subnets[$subnet_id] = {\"type\": \"$type\"}" $ci_file

        elif is_dhcpv6; then
            if $recognize_ipv6_types; then
                type=ipv6_dhcpv6-stateful
            else
                type=dhcp6
            fi
            yq -i ".network.config[$config_id].subnets[$subnet_id] = {\"type\": \"$type\"}" $ci_file

        elif is_staticv6; then
            get_netconf_to ipv6_addr
            get_netconf_to ipv6_gateway
            if $recognize_static6; then
                type_ipv6_static=static6
            else
                type_ipv6_static=static
            fi
            yq -i ".network.config[$config_id].subnets[$subnet_id] = {
                    \"type\": \"$type_ipv6_static\",
                    \"address\": \"$ipv6_addr\",
                    \"gateway\": \"$ipv6_gateway\" }
                    " $ci_file
        fi
        # Unable to set autoconf = false ?
        if should_disable_accept_ra; then
            yq -i ".network.config[$config_id].accept-ra = false" $ci_file
        fi

        # have ipv6 But need to set dns situation
        if is_need_manual_set_dnsv6; then
            need_set_dns6=true
            for cur in $(get_current_dns 6); do
                yq -i ".network.config[$config_id].subnets[$subnet_id].dns_nameservers += [\"$cur\"]" $ci_file
            done
        fi

        config_id=$((config_id + 1))
    done

    if $need_set_dns4 || $need_set_dns6; then
        yq -i ".network.config[$config_id].type=\"nameserver\"" $ci_file
        if $need_set_dns4; then
            for cur in $(get_current_dns 4); do
                yq -i ".network.config[$config_id].address += [\"$cur\"]" $ci_file
            done
        fi
        if $need_set_dns6; then
            for cur in $(get_current_dns 6); do
                yq -i ".network.config[$config_id].address += [\"$cur\"]" $ci_file
            done
        fi
        # if network.config[$config_id] No address，Then delete it to avoid low version cloud-init Report an error
        yq -i "del(.network.config[$config_id] | select(has(\"address\") | not))" $ci_file
    fi

    apk del yq-go

    # View files
    info "Cloud-init network config"
    cat -n $ci_file >&2
}

# The actual test is useless, it is generated machine-id is fixed
# and lightsail centos 9 template machine-id Also the same, obviously the same id not a problem
clear_machine_id() {
    local os_dir=$1

    # https://www.freedesktop.org/software/systemd/man/latest/machine-id.html
    # gentoo The file will not be automatically created
    echo uninitialized >$os_dir/etc/machine-id

    # https://build.opensuse.org/projects/Virtualization:Appliances:Images:openSUSE-Leap-15.5/packages/kiwi-templates-Minimal/files/config.sh?expand=1
    rm -f $os_dir/var/lib/systemd/random-seed
}

is_file_or_link() {
    # -e / -f Bad soft connection, return false
    # -L Bad soft connection, return true
    [ -f $1 ] || [ -L $1 ]
}

cp_resolv_conf() {
    local os_dir=$1
    if is_file_or_link $os_dir/etc/resolv.conf &&
        ! is_file_or_link $os_dir/etc/resolv.conf.orig; then
        mv $os_dir/etc/resolv.conf $os_dir/etc/resolv.conf.orig
    fi
    cp -f /etc/resolv.conf $os_dir/etc/resolv.conf
}

rm_resolv_conf() {
    local os_dir=$1
    rm -f $os_dir/etc/resolv.conf $os_dir/etc/resolv.conf.orig
}

restore_resolv_conf() {
    local os_dir=$1
    if is_file_or_link $os_dir/etc/resolv.conf.orig; then
        mv -f $os_dir/etc/resolv.conf.orig $os_dir/etc/resolv.conf
    fi
}

keep_now_resolv_conf() {
    local os_dir=$1
    rm -f $os_dir/etc/resolv.conf.orig
}

# copy https://github.com/alpinelinux/alpine-conf/blob/3.18.1/setup-disk.in#L421
get_alpine_firmware_pkgs() {
    # must have modloop，otherwise modinfo Will report an error
    ensure_service_started modloop >&2

    # If not in a separate folder, use linux-firmware-other
    # If in a separate folder, use linux-firmware-xxx
    # if not needed firmware，Then use linux-firmware-none
    firmware_pkgs=$(
        cd /sys/module && modinfo -F firmware -- * 2>/dev/null |
            awk -F/ '{print $1 == $0 ? "linux-firmware-other" : "linux-firmware-"$1}' |
            sort -u
    )

    # use command Because I covered it apk added >&2
    retry 5 command apk search --quiet --exact ${firmware_pkgs:-linux-firmware-none}
}

get_ucode_firmware_pkgs() {
    is_virt && return

    case "$distro" in
    *) os=$distro ;;
    esac

    case "$os-$(get_cpu_vendor)" in
    # alpine of linux-firmware Split into folders
    # setup-alpine Will automatically install the required firmware（modloop It will be invalid if it is not mounted.）
    # https://github.com/alpinelinux/alpine-conf/blob/3.18.1/setup-disk.in#L421
    alpine-intel) echo intel-ucode ;;
    alpine-amd) echo amd-ucode ;;
    alpine-*) ;;

    debian-intel) echo firmware-linux intel-microcode ;;
    debian-amd) echo firmware-linux amd64-microcode ;;
    debian-*) echo firmware-linux ;;

    arch-intel) echo linux-firmware intel-ucode ;;
    arch-amd) echo linux-firmware amd-ucode ;;
    arch-*) echo linux-firmware ;;

    gentoo-intel) echo linux-firmware intel-microcode ;;
    gentoo-amd) echo linux-firmware ;;
    gentoo-*) echo linux-firmware ;;

    elol-intel) echo linux-firmware microcode_ctl ;;
    elol-amd) echo linux-firmware microcode_ctl ;;
    elol-*) echo linux-firmware microcode_ctl ;;
    esac
}

del_user_password_and_lock() {
    local os_dir=$1
    local username=$2

    # after locking the user ssh Can I log in?
    # alpine ×
    # Other systems √

    # root Empty password, no lock root，For other users su - root Can I switch to root
    # alpine ×
    # Other systems √

    # centos 7 One line of commands is not supported at the same time -d -l
    # passwd: Only one of -l, -u, -d, -S may be specified.

    # delete password
    chroot "$os_dir" passwd -d "$username"

    # lock the user
    if ! [ -e "$os_dir/etc/alpine-release" ]; then
        chroot "$os_dir" passwd -l "$username"
    fi

    # alpine locked users cannot log in ssh
    # because alpine not enabled by default pam
    # Enabled by default on other systems

    # Not open pam If so, the locked user will be unable to log in ssh
    # turn on pam Later you can

    # alpine is installed by openssh-server-pam turn on pam
    # No settings required UsePAM yes also cannot be recognized UsePAM yes
    # localhost:~# sshd -G | grep -i pam
    # /etc/ssh/sshd_config line 88: Unsupported option UsePAM
}

set_ssh_keys_and_del_password() {
    local os_dir=$1

    info 'set ssh keys'

    if [ "$username" = root ]; then
        local user_home="/root"
    else
        local user_home="/home/$username"
    fi

    # Add public key
    if true; then
        (
            umask 077
            mkdir -p "$os_dir/$user_home/.ssh"
            cat /configs/ssh_keys >"$os_dir/$user_home/.ssh/authorized_keys"
        )
        # Pay attention to use chroot，otherwise uid/gid yes alpine live os down uid/gid
        chroot "$os_dir" chown "$username:$username" "$user_home"
        chroot "$os_dir" chown "$username:$username" "$user_home/.ssh"
        chroot "$os_dir" chown "$username:$username" "$user_home/.ssh/authorized_keys"
    else
        (
            # If added later bsd Unable chroot It can be like this
            umask 077
            read -r owner group < \
                <(awk -F: -v user="$username" '$1==user {print $3,$4}' "$os_dir/etc/passwd")
            install -D \
                -m 600 \
                -o "$owner" \
                -g "$group" \
                /configs/ssh_keys \
                "$os_dir/$user_home/.ssh/authorized_keys"
        )
    fi

    # delete password/lock the user
    del_user_password_and_lock "$os_dir" "$username"

    # debian Cloud mirror /etc/shadow of root The entry is
    # root:!unprovisioned:20591:0:99999:7:::
    # first boot pauses for configuration of root Password interface and blocked ssh Serve
    # So clear it manually here root Password and lock
    if ! [ "$username" = root ] && is_have_cmd_on_disk "$os_dir" systemd-firstboot; then
        del_user_password_and_lock "$os_dir" root
    fi
}

is_ssh_kv_effective() {
    local os_dir=$1
    local key=$2
    local value=$3

    # solve ubuntu 22.04 Report an error
    # Missing privilege separation directory: /run/sshd
    if [ -d "$os_dir/run/sshd" ]; then
        we_create_run_sshd_dir=false
    else
        we_create_run_sshd_dir=true
        mkdir -p "$os_dir/run/sshd"
    fi

    # centos 7 / ubuntu 22.04 Not supported -G
    # -G Only detect configuration files
    # -T Configuration files will be detected、host key
    if res=$(chroot "$os_dir" sshd -G 2>/dev/null || chroot "$os_dir" sshd -T 2>/dev/null); then
        # Delete the ones you created to avoid inaccurate subsequent permissions
        if $we_create_run_sshd_dir; then
            rm -rf "$os_dir/run/sshd"
        fi

        # centos 7 set up prohibit-password ，sshd -T will be displayed as without-password
        printf "%s\n" "$res" |
            sed 's/^permitrootlogin without-password$/permitrootlogin prohibit-password/i' |
            if [ -n "$value" ]; then
                grep -F -xiq "$key $value"
            else
                # value When empty, only verify key exists
                grep -E -xiq "$key .*"
            fi
    else
        error_and_exit "Failed to verify sshd config."
    fi
}

change_ssh_conf_if_different() {
    local os_dir=$1
    local key=$2
    local value=$3
    local explicit=${4:-false} # Does it need to be set explicitly?

    # Some distributions come with certain configurations, such as
    # ubuntu:
    # cat /etc/ssh/sshd_config.d/60-cloudimg-settings.conf | grep -i PasswordAuthentication
    # PasswordAuthentication no

    # gentoo:
    # cat /etc/ssh/sshd_config.d/9999999gentoo-pam.conf | grep -i PasswordAuthentication
    # PasswordAuthentication no

    # 0. If this configuration already exists and does not need to be set explicitly, do not modify it.
    if is_ssh_kv_effective "$os_dir" "$key" "$value" && ! $explicit; then
        return
    fi

    if line="^$key .*" && grep -Exiq "$line" $os_dir/etc/ssh/sshd_config 2>/dev/null; then
        # 1. if sshd_config exists this key（uncommented state), replace
        sed -Ei "s/$line/$key $value/" $os_dir/etc/ssh/sshd_config
    elif include_line='^Include .*/etc/ssh/sshd_config.d' &&
        # 2. if sshd_config Read is set sshd_config.d
        #    then write to sshd_config.d/01-xxx.conf

        # arch No /etc/ssh/sshd_config.d/ folder
        # opensuse tumbleweed No /etc/ssh/sshd_config
        #                       have /etc/ssh/sshd_config.d/ folder
        #                       have /usr/etc/ssh/sshd_config
        { grep -iq "$include_line" $os_dir/etc/ssh/sshd_config ||
            grep -iq "$include_line" $os_dir/usr/etc/ssh/sshd_config; } 2>/dev/null; then
        mkdir -p $os_dir/etc/ssh/sshd_config.d/
        echo "$key $value" >"$os_dir/etc/ssh/sshd_config.d/01-$(echo "$key" | to_lower).conf"
    else
        # 3. write sshd_config
        #    if sshd_config exists this key (Whether it is annotated or not)，then replace, including deleting comments
        #    Otherwise append
        line="^[# ]*$key .*"
        if grep -Exiq "$line" $os_dir/etc/ssh/sshd_config; then
            sed -Ei "s/$line/$key $value/" $os_dir/etc/ssh/sshd_config
        else
            echo "$key $value" >>$os_dir/etc/ssh/sshd_config
        fi
    fi

    # verify success
    if ! is_ssh_kv_effective "$os_dir" "$key" "$value"; then
        error_and_exit "Failed to set sshd config $key $value."
    fi
}

change_ssh_conf_for_key_login() {
    local os_dir=$1

    change_ssh_conf_if_different "$os_dir" PasswordAuthentication no

    # centos 7 PermitRootLogin default is yes，instead of prohibit-password
    if [ "$username" = root ]; then
        change_ssh_conf_if_different "$os_dir" PermitRootLogin prohibit-password
    fi

    # sshd -G/-T have ChallengeResponseAuthentication Description is an old version sshd
    # Only need to set ChallengeResponseAuthentication no

    # OpenSSH 8.6 and below (include el8/debian 11/ubuntu 20.04 wait)
    # KbdInteractiveAuthentication ChallengeResponseAuthentication Can be set to different values
    # If not explicitly set ChallengeResponseAuthentication no
    # but KbdInteractiveAuthentication no will not take effect (sshd -G/-T show KbdInteractiveAuthentication yes)

    # So first sshd -G/-T Check if there is ChallengeResponseAuthentication this key
    # Whenever possible, set it explicitly to no
    # And you need to set it first ChallengeResponseAuthentication post settings KbdInteractiveAuthentication
    # otherwise change_ssh_conf_if_different set up KbdInteractiveAuthentication no It will be detected that it is not effective and an error will be reported.

    # Passed in by user rhel-like The system may support ChallengeResponseAuthentication old version of
    # So even if el8/debian 11/ubuntu 20.04 all EOL This cannot be deleted later
    if is_ssh_kv_effective "$os_dir" ChallengeResponseAuthentication; then
        change_ssh_conf_if_different "$os_dir" ChallengeResponseAuthentication no true
    fi

    # PasswordAuthentication no
    # KbdInteractiveAuthentication yes (default is yes)
    # password login is available in this case
    # ssh -o PreferredAuthentications=keyboard-interactive user@ip

    # Most distributions will have sshd_config set to no
    # but opensuse 16 No
    change_ssh_conf_if_different "$os_dir" KbdInteractiveAuthentication no
}

change_ssh_conf_for_password_login() {
    local os_dir=$1

    # opensuse 16/tumbleweed Install openssh-server-config-rootlogin
    # will generate /usr/etc/ssh/sshd_config.d/50-permit-root-login.conf
    # However, if the user deletes this file, the file may be re-created if the package is updated.？
    # So don’t use this method for now

    # PasswordAuthentication default is yes
    # But some distributions will sshd_config.d set within PasswordAuthentication no
    change_ssh_conf_if_different "$os_dir" PasswordAuthentication yes

    if [ "$username" = root ]; then
        change_ssh_conf_if_different "$os_dir" PermitRootLogin yes
    fi
}

change_ssh_port() {
    local os_dir=$1
    local ssh_port=$2

    change_ssh_conf_if_different "$os_dir" Port "$ssh_port"
}

# Not needed for now
add_user_if_need_for_alpine() {
    local os_dir=$1

    if ! grep -q "^$username:" "$os_dir/etc/passwd"; then
        #  -a  Create admin user. Add to wheel group and set up doas
        #  -u  Unlock the user automatically (eg. creating the user non-interactively
        #      with an ssh key for login)
        if is_need_set_ssh_keys; then
            chroot "$os_dir" setup-user -a -u -k "$(cat /configs/ssh_keys)" "$username"
        else
            chroot "$os_dir" setup-user -a -u "$username"
            change_user_password $os_dir
        fi
    fi
}

add_user_if_need() {
    local os_dir=$1

    # Add user
    if ! grep -q "^$username:" "$os_dir/etc/passwd"; then
        # debian Recommended adduser instead of useradd
        # https://manpages.debian.org/trixie/passwd/useradd.8.en.html
        # useradd is a low level utility for adding users.
        # On Debian, administrators should usually use adduser(8) instead.

        # adduser will follow /etc/adduser.conf Read the default group to be added
        # However usually this value is blank

        # alpine
        if is_have_cmd_on_disk "$os_dir" adduser &&
            chroot "$os_dir" adduser --help 2>&1 | grep -Fq -- BusyBox; then
            chroot "$os_dir" adduser --disabled-password "$username"

        # new version debian/ubuntu
        elif is_have_cmd_on_disk "$os_dir" adduser &&
            chroot "$os_dir" adduser --help 2>&1 | grep -Fq -- '--disabled-password' &&
            chroot "$os_dir" adduser --help 2>&1 | grep -Fq -- '--comment'; then
            chroot "$os_dir" adduser --disabled-password --comment '' "$username"

        # Old version debian/ubuntu
        elif is_have_cmd_on_disk "$os_dir" adduser &&
            chroot "$os_dir" adduser --help 2>&1 | grep -Fq -- '--disabled-password' &&
            chroot "$os_dir" adduser --help 2>&1 | grep -Fq -- '--gecos'; then
            chroot "$os_dir" adduser --disabled-password --gecos '' "$username"

        # el
        elif is_have_cmd_on_disk "$os_dir" adduser &&
            chroot "$os_dir" adduser --help 2>&1 | grep -Fq -- '--password'; then
            chroot "$os_dir" adduser --password ! "$username"

        # arch/gentoo absent by default adduser
        else
            chroot "$os_dir" useradd -m "$username"
        fi
    fi

    # add to wheel/sudo Group
    if ! [ "$username" = root ]; then
        if [ -e "$os_dir/etc/alpine-release" ]; then
            # alpine
            # https://github.com/alpinelinux/alpine-conf/blob/master/setup-user.in#L168

            # Install doas
            chroot "$os_dir" apk add doas doas-sudo-shim
            mkdir -p "$os_dir/etc/doas.d"

            # Add user to group
            chroot "$os_dir" addgroup "$username" wheel

            # doas: Add to wheel Group
            local file="$os_dir/etc/doas.d/20-wheel.conf"
            local content="permit persist :wheel"
            if ! grep -q "^$content" "$file" 2>/dev/null; then
                echo "$content" >>"$file"
            fi

            # doas: Add a single user nopass
            echo "permit nopass $username" >"$os_dir/etc/doas.d/99-$username.conf"
        else
            # Usually used wheel Group
            # debian/ubuntu No wheel group, only sudo Group

            # aws lightsail Test which groups the default user has joined
            # debian       admin : admin adm dialout cdrom floppy sudo audio dip video plugdev
            # ubuntu       ubuntu : ubuntu adm cdrom sudo dip lxd
            # almalinux    ec2-user : ec2-user adm systemd-journal
            # opensuse     ec2-user : ec2-user

            # Add user to group
            for group in \
                wheel sudo \
                adm dialout cdrom floppy audio dip video plugdev lxd systemd-journal; do
                if grep -q "^$group:" "$os_dir/etc/group"; then
                    # chroot "$os_dir" addgroup "$username" "$group"
                    chroot "$os_dir" usermod -aG "$group" "$username"
                fi
            done

            # sudo: gentoo Install sudo Not even after /etc/sudoers.d
            if ! [ -d "$os_dir/etc/sudoers.d" ]; then
                install -d -m 0750 "$os_dir/etc/sudoers.d"
            fi

            # sudo: Add a single user NOPASSWD
            # https://wiki.archlinux.org/title/Sudo#Sudoers_default_file_permissions
            local file="$os_dir/etc/sudoers.d/99-$username"
            printf '%s\n' "$username ALL=(ALL) NOPASSWD:ALL" >"$file"
            chmod 0440 "$file"
        fi
    fi
}

change_user_password() {
    local os_dir=$1

    info 'change user password'

    if is_password_plaintext; then
        pam_d=$os_dir/etc/pam.d

        [ -f $pam_d/chpasswd ] && has_pamd_chpasswd=true || has_pamd_chpasswd=false

        if $has_pamd_chpasswd; then
            cp $pam_d/chpasswd $pam_d/chpasswd.orig

            # cat /etc/pam.d/chpasswd
            # @include common-password

            # cat /etc/pam.d/chpasswd
            # #%PAM-1.0
            # auth       include      system-auth
            # account    include      system-auth
            # password   substack     system-auth
            # -password   optional    pam_gnome_keyring.so use_authtok
            # password   substack     postlogin

            # pass /etc/pam.d/chpasswd turn up /etc/pam.d/system-auth or /etc/pam.d/system-auth
            # Find again password and pam_unix.so line and delete use_authtok，write /etc/pam.d/chpasswd
            files=$(grep -E '^(password|@include)' $pam_d/chpasswd | awk '{print $NF}' | sort -u)
            for file in $files; do
                if [ -f "$pam_d/$file" ] && line=$(grep ^password "$pam_d/$file" | grep -F pam_unix.so); then
                    echo "$line" | sed 's/use_authtok//' >$pam_d/chpasswd
                    break
                fi
            done
        fi

        # Write it in two lines, otherwise it will not terminate when an error occurs.
        plaintext=$(get_password_plaintext)
        printf '%s\n' "$username:$plaintext" | chroot $os_dir chpasswd

        if $has_pamd_chpasswd; then
            mv $pam_d/chpasswd.orig $pam_d/chpasswd
        fi
    else
        printf '%s\n' "$username:$(get_password_linux_sha512)" | chroot $os_dir chpasswd -e
    fi
}

# Applicable to DD Post-repair gpt Backup partition table

mount_part_basic_layout() {
    local os_dir=$1
    local efi_dir=$2

    if is_efi || is_xda_gt_2t; then
        os_part_num=2
    else
        os_part_num=1
    fi

    # Mount system partition
    mkdir -p $os_dir
    mount -t ext4 "/dev/$(xda $os_part_num)" $os_dir

    # mount efi Partition
    if is_efi; then
        mkdir -p $efi_dir
        mount -t vfat -o umask=077 "/dev/$(xda 1)" $efi_dir
    fi
}

# virt-what Use the latest version
# vultr 1G High Frequency LAX actually kvm
# debian 11 virt-what 1.19 displayed as hyperv qemu
# debian 11 systemd-detect-virt displayed as microsoft
# alpine virt-what 1.25 displayed as kvm
# So don’t judge the specific virtualization environment on the original system.

# lscpu You can also view virtualized environments, but alpine on lightsail The running result is Microsoft
# guess lscpu Just for reference cpuid No reference dmi
# virt-what Multiple lines of results may be output, so use grep

# Will AC/SAC version number converted to LTSC version number
# Used to find drivers

# reinstall.sh There is a method with the same name

mkdir_clear() {
    local dir=$1

    if [ -z "$dir" ] || [ "$dir" = / ]; then
        return
    fi

    rm -rf "$dir"
    mkdir -p "$dir"
}

# Note that the usage method is list=$(list_add "$list" "$item_to_add")

# reinstall.sh There is a method with the same name

# Add to netboot.efi spare

sync_time() {

    # ntp If the time difference is too large, synchronization will be impossible.？
    # http The time may not be accurate. After all, it is not a specialized time server.
    #      Maybe not date header?
    method=http

    case "$method" in
    ntp)
        ntp_server=pool.ntp.org
        # -d[d]   Verbose
        # -n      Run in foreground
        # -q      Quit after clock is set
        # -p      PEER
        ntpd -d -n -q -p "$ntp_server"
        ;;
    http)
        url="$(grep -m1 ^http /etc/apk/repositories)/$(uname -m)/APKINDEX.tar.gz"
        # There may be multiple lines, take the first line
        date_header=$(wget -S --no-check-certificate --spider "$url" 2>&1 | grep -m1 '^  Date:')
        # gnu date Not supported -D
        busybox date -u -D "  Date: %a, %d %b %Y %H:%M:%S GMT" -s "$date_header"
        ;;
    esac

    # during reboot alpine It will be automatically written to the hardware clock, so skip it here.
    # hwclock -w
}

is_tencent_cloud() {
    [ "$(cat /sys/devices/virtual/dmi/id/sys_vendor 2>/dev/null)" = 'Tencent Cloud' ]
}

add_max_sectors_kb_rule() {
    local os_dir=$1

    # Normal distribution
    if [ -d $os_dir/etc/udev/rules.d/ ]; then
        # Taken from Tencent Cloud ubuntu 26.04 image
        cat <<EOF >$os_dir/etc/udev/rules.d/80-max-sectors-blk.rules
KERNEL=="vd*[a-z]", ACTION=="add|change", SUBSYSTEM=="block", RUN+="/bin/sh -c '/bin/echo 512 > /sys/%p/queue/max_sectors_kb'"
EOF

    # alpine
    elif [ -f $os_dir/etc/mdev.conf ]; then
        if ! grep -Eq '^vd.*max_sectors_kb' "$os_dir/etc/mdev.conf"; then
            # shellcheck disable=SC2016
            sed -Ei \
                '/^vd\[a-z\]/s,$,; case "$ACTION" in add|change) if [[ "$MDEV" =~ [a-z]$ ]]; then echo 512 >/sys/class/block/$MDEV/queue/max_sectors_kb; fi;; esac,' \
                "$os_dir/etc/mdev.conf"
        fi
    fi

}

set_max_sectors_kb_for_tencent_cloud_liveos() {
    if is_tencent_cloud; then
        local block
        for block in /sys/class/block/vd[a-z]; do
            if [ -d "$block" ]; then
                echo 512 >"$block/queue/max_sectors_kb"
            fi
        done
        add_max_sectors_kb_rule /
        rc-service mdev restart
        sleep 1
        # update_part
    fi
}

set_max_sectors_kb_for_tencent_cloud_persist() {
    local os_dir etc_dir
    if is_tencent_cloud && etc_dir=$({ ls -d /os/etc/ || ls -d /os/*/etc/; } 2>/dev/null); then
        os_dir=$(dirname $etc_dir)
        # remount read-write
        mount -o remount,rw /os

        # rule
        add_max_sectors_kb_rule "$os_dir"

        # regenerate initramfs
        # el
        if is_have_cmd_on_disk $os_dir dracut; then
            chroot $os_dir dracut -f --regenerate-all
        # debian/ubuntu
        elif is_have_cmd_on_disk $os_dir update-initramfs; then
            chroot $os_dir update-initramfs -u -k all
        # arch
        elif is_have_cmd_on_disk $os_dir mkinitcpio; then
            echo 'FILES+=(/etc/udev/rules.d/80-max-sectors-blk.rules)' \
                >$os_dir/etc/mkinitcpio.conf.d/80-max-sectors-blk.conf
            chroot $os_dir mkinitcpio -P
        # alpine
        elif is_have_cmd_on_disk $os_dir mkinitfs; then
            chroot $os_dir mkinitfs
        fi

    fi
}

minimum_ram_for_target() {
    case "$1" in
    alpine | debian) echo 512 ;;
    arch) echo 1024 ;;
    gentoo) echo 2048 ;;
    *) error_and_exit "Cannot determine RAM floor for $1." ;;
    esac
}

check_install_ram() {
    local min_ram actual_ram
    min_ram=$(minimum_ram_for_target "$distro")
    actual_ram=$(get_approximate_ram_size)
    case "$actual_ram" in
    '' | *[!0-9]*) error_and_exit "Could not determine RAM size in Alpine Live." ;;
    esac
    [ "$actual_ram" -ge "$min_ram" ] ||
        error_and_exit "Could not install $distro: RAM < $min_ram MB (detected $actual_ram MB)."
}

validate_target() {
    case "$distro" in
    alpine | debian | arch | gentoo) ;;
    *) error_and_exit "Unsupported target in trimmed runtime: $distro" ;;
    esac
}

trans() {
    validate_target
    check_install_ram
    info "start trans"
    mod_motd
    ensure_service_started modloop
    cat /proc/cmdline
    clear_previous
    add_community_repo
    if [ -z "$xda" ]; then find_xda; fi
    set_max_sectors_kb_for_tencent_cloud_liveos
    if [ "$distro" != alpine ]; then
        setup_web_if_enough_ram
        apk add util-linux
    fi
    case "$distro" in
    alpine) install_alpine ;;
    debian)
        create_part
        install_debian
        ;;
    arch | gentoo)
        create_part
        install_arch_gentoo
        ;;
    esac
    set_max_sectors_kb_for_tencent_cloud_persist
    if is_efi; then
        del_invalid_efi_entry
        add_default_efi_to_nvram
    fi
    info 'done'
    sleep 5
}

# Script entry

[ "$(uname -m)" = x86_64 ] || error_and_exit "Alpine Live installer requires x86_64."

# copy script
# Used to print errors or run again
# If the paths are the same, there is no need to copy them.
# Important: copy before deleting the script
if ! [ "$(readlink -f "$0")" = /trans.sh ]; then
    cp -f "$0" /trans.sh
fi
trap 'trap_err $LINENO $?' ERR

# Delete this script, otherwise it will be copied to the new system
rm -f /etc/local.d/trans.start
rm -f /etc/runlevels/default/local

# Extract variables
extract_env_from_cmdline

# Run section with parameters
# redownload and exec Run new script
if [ "$1" = "update" ]; then
    info 'update script'
    # shellcheck disable=SC2154
    wget -O /trans.sh.update "$confhome/trans.sh"
    grep -qF "SCRIPT_VERSION=$SCRIPT_VERSION" /trans.sh.update || error_and_exit "Runtime version mismatch; refusing update."
    [ -s /trans.sh.update ] || error_and_exit "Empty runtime update."
    chmod +x /trans.sh.update
    mv -f /trans.sh.update /trans.sh
    exec /trans.sh
elif [ "$1" = "alpine" ]; then
    info 'switch to alpine'
    distro=alpine
elif [ -n "$1" ]; then
    error_and_exit "unknown option $1"
fi

validate_target

# Run section without parameters
# allow ramdisk Use all memory, default is 50%
mount / -o remount,size=100%

# sync time
# 1. can prevent access https Error
# 2. can prevent https://github.com/bin456789/reinstall/issues/223
#    E: Release file for http://security.ubuntu.com/ubuntu/dists/noble-security/InRelease is not valid yet (invalid for another 5h 37min 18s).
#    Updates for this repository will not be applied.
# 4. Allow synchronization to fail because it is not a critical step
sync_time || true

# Install ssh and change the port
apk add openssh-server
if is_need_change_ssh_port; then
    change_ssh_port / $ssh_port
fi

# Set password and add startup + turn on ssh Serve
add_user_if_need /
if is_need_set_ssh_keys; then
    set_ssh_keys_and_del_password /
    change_ssh_conf_for_key_login /
    printf '\n' | setup-sshd
else
    change_user_password /
    change_ssh_conf_for_password_login /
    printf '\nyes' | setup-sshd
fi

# set up frpc
# and prevent repeated runs
if ls /configs/frpc.* >/dev/null 2>&1 && ! pidof frpc >/dev/null; then
    info 'run frpc'
    chmod 600 /configs/frpc.*
    add_community_repo
    apk add frp
    while true; do
        frpc -c /configs/frpc.* || true
        sleep 5
    done &
fi

# shellcheck disable=SC2154
if [ "$hold" = 1 ]; then
    if is_run_from_locald; then
        info "hold"
        exit
    fi
fi

# Officially run reinstallation
# shellcheck disable=SC2046,SC2194
case 1 in
1)
    # ChatGPT said to provide the best performance
    exec > >(exec tee $(get_ttys /dev/) /reinstall.log) 2>&1
    trans
    ;;
2)
    exec > >(tee $(get_ttys /dev/) /reinstall.log) 2>&1
    trans
    ;;
3)
    trans 2>&1 | tee $(get_ttys /dev/) /reinstall.log
    ;;
esac

if [ "$hold" = 2 ]; then
    info "hold 2"
    exit
fi

# umount ?
sync
reboot
