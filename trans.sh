#!/bin/ash
# Personal Linux-only derivative of bin456789/reinstall; see docs/TRIMMING.md.
# shellcheck shell=dash
# shellcheck disable=SC2086,SC3047,SC3036,SC3010,SC3001,SC3060,SC3015
# alpine 默认使用 busybox ash
# 注意 bash 和 ash 以下语句结果不同
# [[ a = '*a' ]] && echo 1

# 出错后停止运行，将进入到登录界面，防止失联
set -eE

# 用于判断 reinstall.sh 和 trans.sh 是否兼容
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

    # 解除锁定，允许用户登录处理故障
    # passwd -u "$username" >/dev/null

    # 用不着，因为 alpine 锁定账户后无法登录 ssh
    # 因此不会锁定

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

# reinstall.sh 有相同方法 add_community_repo_for_alpine
add_community_repo() {
    local ver mirror

    # 先检查原来的 repo 是不是 edge 或者 latest-stable
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

# 有时网络问题下载失败，导致脚本中断
# 因此需要重试
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
    # killall 是异步的，要等一下
    local ret=0
    if ! command killall "$@"; then
        ret=$?
    fi
    sleep 5
    return $ret
}

# 在没有设置 set +o pipefail 的情况下，限制下载大小：
# retry 5 command wget | head -c 1048576 会触发 retry，下载 5 次
# command wget "$@" --tries=5 | head -c 1048576 不会触发 wget 自带的 retry，只下载 1 次
wget() {
    show_url_in_args "$@" >&2
    if command wget 2>&1 | grep -q BusyBox; then
        # busybox wget 没有重试功能
        # 好像默认永不超时
        retry 5 command wget "$@" -T 10
    else
        # 原版 wget 自带重试功能
        command wget --tries=5 --progress=bar:force "$@"
    fi
}

is_have_cmd() {
    # command -v 包括脚本里面的方法
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
            # wget -O- | grep -m1 成功后会提前关闭管道，导致 141 错误
            # 这是预期行为，因此需要排除
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

    # 复制当前系统的 /etc/apk 文件夹
    mkdir -p "$os_dir"
    cp -a --parents /etc/apk "$os_dir"
    rm -f "$os_dir/etc/apk/world"

    # 安装 alpine
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

    # 将 alpine-base 的依赖写入 world，再删除 alpine-base alpine-conf
    # --installed --depends 顺序不能错
    # 不添加 --installed 则会同时显示已安装的和最新版的
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
    # 有分区挂载中会报 Resource busy 错误
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
    # mdev 不会删除 /dev/disk/ 的旧分区，因此手动删除
    # 如果 rm -rf 的时候刚好 mdev 在创建链接，rm -rf 会报错 Directory not empty
    # 因此要先停止 mdev 服务
    # 还要删除 /dev/$xda*?
    ensure_service_stopped mdev
    sleep 1
    # 即使停止了 mdev，有时也会报 Directory not empty，因此添加 retry
    retry 5 rm -rf /dev/disk/*

    # 没挂载 modloop 时会提示
    # modprobe: can't change directory to '/lib/modules': No such file or directory
    # 因此强制不显示上面的提示
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
    # websocketd 遇到 \n 才推送，因此要转换 \r 为 \n
    websocketd --port "$web_port" --loglevel=fatal --staticdir=/tmp/web \
        stdbuf -oL -eL \
        sh -c "if [ \"\$PATH_INFO\" = \"$web_path\" ]; then tail -fn+0 /reinstall.log | tr '\r' '\n' | grep -Fiv -e password -e token; fi" &
}

get_approximate_ram_size() {
    # lsmem 需要 util-linux

    if [ -z $ram_size ]; then
        ram_size=$(free -m | awk '{print $2}' | sed -n '2p')
    fi

    echo "$ram_size"
}

setup_web_if_enough_ram() {
    total_ram=$(get_approximate_ram_size)
    # 512内存才安装
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

    # 最后一个 tty 是主 tty，显示的信息最全
    # hytron 有 ttyS0 但无法写入；用于 cmdline 时不能排除不存在的 tty
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
    # 出错后再运行脚本，硬盘可能已经格式化，之前记录的分区表 id 无效
    # 因此找到 xda 后要保存 xda 到 /configs/xda

    # 先读取之前保存的
    if xda=$(get_config xda 2>/dev/null) && [ -n "$xda" ]; then
        return
    fi

    # 防止 $main_disk 为空
    if [ -z "$main_disk" ]; then
        error_and_exit "cmdline main_disk is empty."
    fi

    # busybox fdisk/lsblk/blkid 不显示 mbr 分区表 id
    # 可用以下工具：
    # fdisk 在 util-linux-misc 里面，占用大
    # sfdisk 占用小
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
    # 提取 finalos/extra 到变量
    for prefix in finalos extra; do
        while read -r line; do
            if [ -n "$line" ]; then
                key=$(echo $line | cut -d= -f1)
                value=$(echo $line | cut -d= -f2-)
                eval "$key='$value'"
            fi
        done < <(xargs -n1 </proc/cmdline | grep "^${prefix}_" | sed "s/^${prefix}_//")
    done

    # 如果空白则设置默认值
    username=${username:-root}
    ssh_port=${ssh_port:-22}
    web_port=${web_port:-80}
}

ensure_service_started() {
    local service=$1

    if ! rc-service -q "$service" start; then
        for i in $(seq 10); do
            if [ "$service" = modloop ]; then
                # 避免有时 modloop 下载不完整导致报错
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
    # 安装后 alpine 后要恢复默认
    # 自动安装失败后，可能手动安装 alpine，因此无需判断 $distro
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

# 可能脚本不是首次运行，先清理之前的残留
clear_previous() {
    if is_have_cmd vgchange; then
        umount -R /os || true
        vgchange -an
        apk add device-mapper
        dmsetup remove_all
    fi
    # 安装 arch 有 gpg-agent 进程驻留
    # 在 aria2c 下载时手动中止脚本，aria2c 还会在后台下载
    killall -q gpg-agent aria2c || true
    umount_all

    # 以下情况 umount -R /1 会提示 busy
    # mount /file1 /1
    # mount /1/file2 /2
}

# virt-what 自动安装 dmidecode，因此同时缓存
cache_dmi_and_virt() {
    if ! [ "$_dmi_and_virt_cached" = 1 ]; then
        apk add virt-what

        # 区分 kvm 和 virtio，原因:
        # 1. 阿里云 c8y virt-what 不显示 kvm
        # 2. 不是所有 kvm 都需要 virtio 驱动，例如 aws nitro
        # 3. virt-what 不会检测 virtio
        _virt=$(
            virt-what

            # hyper-v 环境下 modprobe virtio_scsi 也会创建 /sys/bus/virtio/drivers/virtio_scsi
            # 因此用 devices 判断更准确，有设备时才有 /sys/bus/virtio/drivers/*
            # 或者加上 lspci 检测?

            # 不要用 ls /sys/bus/virtio/devices/* && echo virtio
            # 因为有可能返回值不为 0 而中断脚本
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
        # 有时会重复收取，所以设置收一份后退出
        echo "Gathering network info..."
        # shellcheck disable=SC2154
        _ra="$(rdisc6 -1 "$ethx")"
        apk del ndisc6

        # 显示网络配置
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

# 有 dhcpv4 不等于有网关，例如 vultr 纯 ipv6
# 没有 dhcpv4 不等于是静态ip，可能是没有 ip
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
    # 如果是静态（包括自动获取到 IP 但无法联网而切换成静态）直接返回 1，不考虑 ra
    # 防止部分机器slaac/dhcpv6获取的ip/网关无法上网

    # 有可能 ra 的 dhcpv6/slaac 是打开的，但实测无法获取到 ipv6 地址
    # is_dhcpv6_or_slaac 是实测结果，因此如果实测不通过，也返回 1

    # 不要判断 is_staticv6，因为这会导致死循环
    if ! is_ipv6_has_internet || ! is_dhcpv6_or_slaac || should_disable_accept_ra || should_disable_autoconf; then
        return 1
    fi
    get_netconf_to slaac
    # shellcheck disable=SC2154
    [ "$slaac" = 1 ]
}

is_dhcpv6() {
    # 如果是静态（包括自动获取到 IP 但无法联网而切换成静态）直接返回 1，不考虑 ra
    # 防止部分机器slaac/dhcpv6获取的ip/网关无法上网

    # 有可能 ra 的 dhcpv6/slaac 是打开的，但实测无法获取到 ipv6 地址
    # is_dhcpv6_or_slaac 是实测结果，因此如果实测不通过，也返回 1

    # 不要判断 is_staticv6，因为这会导致死循环
    if ! is_ipv6_has_internet || ! is_dhcpv6_or_slaac || should_disable_accept_ra || should_disable_autoconf; then
        return 1
    fi
    get_netconf_to dhcpv6

    # shellcheck disable=SC2154
    # 甲骨文即使没有添加 IPv6 地址，RA DHCPv6 标志也是开的
    # 部分系统开机需要等 DHCPv6 超时
    # 这种情况需要禁用 DHCPv6
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
    # rdnss 可能有几个
    get_netconf_to rdnss
    [ -n "$rdnss" ]
}

# 15063 或之后才支持 rdnss

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
    # 从第一行直到找到 ^[:space:]
    # 这个区间内删除所有空行
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

# reinstall.sh 有同名方法
grep_efi_index() {
    awk '{print $1}' | sed -e 's/Boot//' -e 's/\*//'
}

# 某些机器可能不会回落到 bootx64.efi
# 阿里云 ECS 启动项有 EFI Shell
# 添加 bootx64.efi 到最后的话，会进入 EFI Shell
# 因此添加到最前面
add_default_efi_to_nvram() {
    info "add default EFI to nvram"

    apk add lsblk efibootmgr

    if efi_row=$(lsblk /dev/$xda -ro NAME,PARTTYPE,PARTUUID | grep -i "$EFI_UUID"); then
        efi_part_uuid=$(echo "$efi_row" | awk '{print $3}')
        efi_part_name=$(echo "$efi_row" | awk '{print $1}')
        efi_part_num=$(get_part_num_by_part "$efi_part_name")
        efi_file=$(get_fallback_efi_file_name)

        # 创建条目，先判断是否已经存在
        # 好像没必要先判断
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
            # reinstall.sh 里确认过一遍，但是逻辑扇区大于 512 时，可能漏报？
            # 这里的应该会根据逻辑扇区来判断？
            echo "
Warning: This machine is currently using EFI boot, but the main hard drive does not have an EFI partition.
If this machine supports Legacy BIOS boot (CSM), you can safely restart into the new system by running the reboot command.
If this machine does not support Legacy BIOS boot (CSM), you will not be able to enter the new system after rebooting.

警告：本机目前使用 EFI 引导，但主硬盘没有 EFI 分区。
如果本机支持 Legacy BIOS 引导 (CSM)，你可以运行 reboot 命令安全地重启到新系统。
如果本机不支持 Legacy BIOS 引导 (CSM)，重启后将无法进入新系统。
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

    # 默认 grep -E
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

    # 生成 lo配置
    cat <<EOF >>$conf_file
auto lo
iface lo inet loopback
EOF

    # ethx
    for ethx in $(get_eths); do
        mode=auto
        # shellcheck disable=SC2154

        # 头部
        get_netconf_to mac_addr
        {
            echo
            # 这是标记，fix-eth-name 要用，不要删除
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
            # 额外的 IPv6 地址（子网不含网关的地址）
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
        # accept_ra/autoconf 属于 iface 选项
        # 如果当前网卡没有生成 IPv6 iface stanza，
        # 先补一个 manual stanza，避免 ifupdown 报 misplaced option
        if ! $has_ipv6_iface &&
            { should_disable_accept_ra || should_disable_autoconf; }; then
            echo "iface $ethx inet6 manual" >>$conf_file
        fi
        # dns
        # 有 ipv6 但需设置 dns 的情况
        if is_need_manual_set_dnsv6; then
            for dns in $(get_current_dns 6); do
                cat <<EOF >>$conf_file
    dns-nameserver $dns
EOF
            done
        fi

        # 禁用 ra
        if should_disable_accept_ra; then
            cat <<EOF >>$conf_file
    pre-up echo 0 >/proc/sys/net/ipv6/conf/$ethx/accept_ra
EOF
        fi

        # 禁用 autoconf
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

# 不够严谨，谨慎使用

install_alpine() {
    info "install alpine"

    # alpine 安装时会自动检测安装需要的 firmware
    # https://github.com/alpinelinux/alpine-conf/blob/3.18.1/setup-disk.in#L421
    # 但如果没有 modloop 则无法检测
    # 所以删除 modloop 前先记录用到的 firmware 包
    fw_pkgs=$(get_alpine_firmware_pkgs)

    # bios机器用 setup-disk 自动分区会有 boot 分区
    # 因此手动分区安装
    create_part
    mount_part_basic_layout /os /os/boot/efi

    # 网络配置
    create_ifupdown_config /etc/network/interfaces
    echo
    cat -n /etc/network/interfaces
    echo

    # The Alpine initramfs may have selected swclock before the target system was mounted.
    rc-update del swclock boot || true
    rc-update add hwclock boot

    # 通过 setup-alpine 安装会启用以下几个服务
    # https://github.com/alpinelinux/alpine-conf/blob/3.18.1/setup-alpine.in#L229

    # boot
    rc-update add networking boot
    rc-update add seedrng boot

    # default
    rc-update add crond
    if [ -e /dev/input/event0 ]; then
        rc-update add acpid
    fi

    # 内核
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

    # 重置为官方仓库配置
    # 国内机可能无法访问mirror列表而报错

    # setup-disk 安装 grub 跳过了添加引导项到 nvram
    # 防止部分机器不会 fallback 到 bootx64.efi
    if is_efi; then
        apk add efibootmgr
        sed -i 's/--no-nvram//' "$(which setup-disk)"
    fi

    # 安装到硬盘
    # alpine默认使用 syslinux (efi 环境除外)，这里强制使用 grub，方便用脚本再次重装
    KERNELOPTS="$(get_ttys console=)"
    export KERNELOPTS
    export BOOTLOADER="grub"
    setup-disk -m sys -k $kernel_flavor /os

    # 删除 setup-disk 时自动安装的包
    apk del e2fsprogs dosfstools efibootmgr grub*

    # 如果没有挂载 /proc

    # 1. chroot /os setup-keymap us us 会报错
    # grep: /proc/filesystems: No such file or directory

    # 2. 安装固件微码会触发 grub-probe，如果没挂载会报错
    # Executing grub-2.12-r5.trigger
    # /usr/sbin/grub-probe: error: failed to get canonical path of `/dev/vda1'.
    # ERROR: grub-2.12-r5.trigger: script exited with error 1

    mount_pseudo_fs /os

    # azure nvme 实例的 initramfs 需要添加 pci_hyperv 驱动
    if [ -d /sys/module/pci_hyperv ] &&
        get_drivers "/sys/block/$xda" | grep -qx pci_hyperv; then
        echo 'kernel/drivers/pci/controller/pci-hyperv.ko*' >/os/etc/mkinitfs/features.d/pci-hyperv.modules
        if ! grep -q 'pci-hyperv' /os/etc/mkinitfs/mkinitfs.conf; then
            # 找到 features=" 开头的行，将最后的"改成 pci-hyperv"
            sed -i '/features="/s/"$/ pci-hyperv"/' /os/etc/mkinitfs/mkinitfs.conf
        fi
        chroot /os mkinitfs -k "$(basename /os/lib/modules/*-*)"
    fi

    # 安装到硬盘后才安装各种应用
    # 避免占用 Live OS 内存

    # 网络
    # udhcpc
    # 坑1 ip -4 addr 无法知道是否是 dhcp
    # 坑2 networking 服务不会运行 udhcpc6
    # 坑3 h3c 移动云电脑 udhcpc6 无法获取 dhcpv6

    # dhcpcd
    # 坑1 slaac默认开了隐私保护，造成ip和后台面板不一致

    # slaac方案1: udhcpc + rdnssd
    # slaac方案2: dhcpcd + 关闭隐私保护
    # dhcpv6方案: dhcpcd

    # 综合使用dhcpcd方案
    # 1 无需改动/etc/network/interfaces，自动根据ra使用slaac和dhcpv6
    # 2 自带rdnss支持
    # 3 唯一要做的是关闭隐私保护

    # 安装 dhcpcd
    chroot /os apk add dhcpcd
    chroot /os sed -i '/^slaac private/s/^/#/' /etc/dhcpcd.conf
    chroot /os sed -i '/^#slaac hwaddr/s/^#//' /etc/dhcpcd.conf

    # 安装其他部件
    chroot /os setup-keymap us us
    chroot /os setup-timezone -i Asia/Shanghai
    # 3.21 默认是 chrony
    # 3.22 默认是 busybox ntp
    printf '\n' | chroot /os setup-ntp || true

    # 设置公钥
    add_user_if_need /os
    if is_need_set_ssh_keys; then
        set_ssh_keys_and_del_password /os
    fi

    # 首次启动修正网卡名
    download "$confhome/fix-eth-name.sh" /os/fix-eth-name.sh
    mkdir -p /os/etc/init.d
    write_fix_eth_name_openrc_service /os/etc/init.d/fix-eth-name
    chmod 755 /os/etc/init.d/fix-eth-name
    chroot /os rc-update add fix-eth-name boot

    # 安装 frpc
    if ls /configs/frpc.* >/dev/null 2>&1; then
        chroot /os apk add frp
        # chroot rc-update add 默认添加到 sysinit
        # 但不加 chroot 默认添加到 default
        chroot /os rc-update add frpc default

        # 固定为 toml
        # 这样就不用在 /etc/init.d/frpc drop-in 修改 cfgfile
        cp -f /configs/frpc.* /os/etc/frp/frpc.toml
        chmod 600 /os/etc/frp/frpc.toml
    fi

    # setup-disk 会自动选择固件，但不包括微码？
    # https://github.com/alpinelinux/alpine-conf/blob/3.18.1/setup-disk.in#L421
    if fw_pkgs="$fw_pkgs $(get_ucode_firmware_pkgs)" && [ -n "$fw_pkgs" ]; then
        chroot /os apk add $fw_pkgs
    fi

    # 3.19 或以上，非 efi 需要手动安装 grub
    if ! is_efi; then
        chroot /os grub-install --target=i386-pc /dev/$xda
    fi

    # efi grub 添加 fwsetup 条目
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

# 设置线程
# 根据 cpu 核数，每个线程的内存，取最小值
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

    # 可能是 /usr/lib/systemd/system-preset/ 或者 /lib/systemd/system-preset/
    if [ -d "$os_dir/usr/lib/systemd/system-preset" ]; then
        echo "enable $service_name.service" >"$os_dir/usr/lib/systemd/system-preset/01-$service_name.preset"
    else
        echo "enable $service_name.service" >"$os_dir/lib/systemd/system-preset/01-$service_name.preset"
    fi
}

add_fix_eth_name_systemd_service() {
    local os_dir=$1

    # 无需执行 systemctl daemon-reload
    # 因为 chroot 下执行会提示 Running in chroot, ignoring command 'daemon-reload'
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

        # 下载 frpc
        # 注意下载的 frpc owner 不是 root:root
        frpc_url=$(get_frpc_url linux)
        basename=$(echo "$frpc_url" | awk -F/ '{print $NF}' | sed 's/\.tar\.gz//')
        download "$frpc_url" "$os_dir/frpc.tar.gz"
        # busybox tar 不支持 wildcard
        # tar: */frpc: not found in archive
        tar xzf "$os_dir/frpc.tar.gz" "$basename/frpc" -O >"$os_dir/usr/local/bin/frpc"
        rm -f "$os_dir/frpc.tar.gz"
        chmod a+x "$os_dir/usr/local/bin/frpc"

        # frpc toml
        cp -f /configs/frpc.* "$os_dir/usr/local/etc/frpc/frpc.toml"
        # Retained systemd targets use LoadCredential (systemd >= 247).
        chroot "$os_dir" chown root:root /usr/local/etc/frpc/frpc.toml
        chroot "$os_dir" chmod 600 /usr/local/etc/frpc/frpc.toml

        # 添加服务
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
        # 删除最后的若干个 /
        mount_point=$(printf "%s" "$mount_point" | sed 's,/*$,,')
    fi

    # findmnt 要安装
    # findmnt "$mount_point" -rno FSTYPE
    mount | awk -v mp="$1" '$3==mp {print $5}' | grep .
}

basic_init() {
    local os_dir=$1

    # 此时不能用
    # chroot $os_dir timedatectl set-timezone Asia/Shanghai
    # Failed to create bus connection: No such file or directory

    if is_have_cmd_on_disk $os_dir systemd-firstboot; then
        if chroot $os_dir systemd-firstboot --help | grep -wq '\--force'; then
            chroot $os_dir systemd-firstboot --timezone=Asia/Shanghai --force
        else
            chroot $os_dir systemd-firstboot --timezone=Asia/Shanghai
        fi
    fi

    # gentoo 不会自动创建 machine-id
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

    # 公钥/密码
    add_user_if_need "$os_dir"
    if is_need_set_ssh_keys; then
        set_ssh_keys_and_del_password $os_dir
        change_ssh_conf_for_key_login $os_dir
    else
        change_user_password $os_dir
        change_ssh_conf_for_password_login $os_dir
    fi

    # 生成 fix-eth-name.service
    # 即使开了 net.ifnames=0 也需要
    # 因为 alpine live 和目标系统的网卡顺序可能不同
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

        # 为了二次运行时 /etc/pacman.conf 未修改
        if [ -f $alpine_rootfs/etc/pacman.conf.orig ]; then
            cp $alpine_rootfs/etc/pacman.conf.orig $alpine_rootfs/etc/pacman.conf
        else
            cp $alpine_rootfs/etc/pacman.conf $alpine_rootfs/etc/pacman.conf.orig
        fi

        # 设置 repo
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

        # 安装系统
        # 要安装分区工具(包含 fsck.xxx)，用于 initramfs 检查分区数据
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

        # retry 防止网络问题
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

        # 挂载伪文件系统
        mount_pseudo_fs $os_dir

        # 要先设置语言，再安装内核，不然出现
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
        # 解压系统
        apk add tar xz pv
        # shellcheck disable=SC2154
        download "$img" $os_dir/gentoo.tar.xz
        echo "Uncompressing Gentoo..."
        pv -f $os_dir/gentoo.tar.xz | tar xpJ --numeric-owner --xattrs-include='*.*' -C $os_dir
        rm $os_dir/gentoo.tar.xz
        apk del tar xz pv

        # dns
        cp_resolv_conf $os_dir

        # 挂载伪文件系统
        mount_pseudo_fs $os_dir

        # 下载仓库，选择 profile
        chroot $os_dir emerge-webrsync
        profile=$(
            # 筛选 stable systemd，再选择最短的
            chroot $os_dir eselect profile list | grep stable | grep systemd |
                awk '{print length($2), $2}' | sort -n | head -1 | awk '{print $2}'
        )
        echo "Select profile: $profile"
        chroot $os_dir eselect profile set $profile

        # 设置 license
        cat <<EOF >>$os_dir/etc/portage/make.conf
ACCEPT_LICENSE="*"
EOF

        cat <<EOF >>$os_dir/etc/portage/make.conf
MAKEOPTS="-j$(get_build_threads 2048)"
EOF

        # 设置 http repo + binpkg repo
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

        # 下载公钥

        # getuto 会判断是否有 ${TERM} 且 ${TERM} 不是 dumb
        # 符合条件则 source /lib/gentoo/functions.sh 导入 ebegin 等方法
        # 不符合条件则自行创建 ebegin 等方法

        # /lib/gentoo/functions.sh 会判断是否有 ${RC_OPENRC_PID}
        # 有的话就不会导入 /functions/openrc.sh，也就不会导入 ebegin 方法

        # 在 ssh 里运行 /trans.sh 时，没有 ${RC_OPENRC_PID}，${TERM} 是 xterm
        # 因此 chroot $os_dir getuto 时不会报错

        # 在 locald 里运行 /trans.sh 时，有 ${RC_OPENRC_PID}，${TERM} 是 linux
        # 因此 chroot $os_dir getuto 时会报错说 ebegin: command not found

        # 有两个解决方法
        if true; then
            TERM=dumb chroot $os_dir getuto
        else
            env -u RC_OPENRC_PID chroot $os_dir getuto
        fi

        set_locale

        # 安装 git 会升级 glibc，此时 /etc/locale.gen 不能为空，否则会提示生成所有 locale
        # Generating all locales; edit /etc/locale.gen to save time/space
        chroot $os_dir emerge dev-vcs/git

        # 设置 git repo
        # github 不支持 ipv6
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

        # 一次性安装所有包，可减少软件包重复 rebuild ?
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

        # 安装 grub + 内核
        is_efi && grub_platforms="efi-64" || grub_platforms="pc"
        echo GRUB_PLATFORMS=\"$grub_platforms\" >>$os_dir/etc/portage/make.conf
        echo "sys-kernel/installkernel dracut grub" >$os_dir/etc/portage/package.use/installkernel

        # 要设置 root=UUID=xxxx，否则 dracut 会报错
        # 要注意 root=UUID=xxxx 头尾有空格
        # https://wiki.gentoo.org/wiki/Installkernel#Install_chroot_check
        # https://wiki.gentoo.org/wiki/Handbook:AMD64/Installation/Kernel#Chroot_detection
        uuid=$(chroot $os_dir findmnt -rno UUID /)
        mkdir -p $os_dir/etc/dracut.conf.d
        echo "kernel_cmdline=\" root=UUID=$uuid \"" >$os_dir/etc/dracut.conf.d/00-installkernel.conf
        pkgs="$pkgs sys-kernel/gentoo-kernel-bin"

        # 安装
        # 不添加 -n/--noreplace 的话会 rebuild 选定的包，无论是否已安装
        chroot "$os_dir" emerge -n $pkgs
    }

    local os_dir=/os

    # 挂载分区
    mount_part_basic_layout /os /os/efi

    # 安装系统
    install_$distro

    # 安装 arch 有 gpg-agent 进程驻留
    killall -q gpg-agent || true

    # 初始化

    # Network configuration is shared by Arch, Gentoo and Debian.
    setup_systemd_networkd "$os_dir"

    basic_init $os_dir

    # ntp 用 systemd 自带的
    # TODO: vm agent + 随机数生成器

    # grub
    if is_efi; then
        # arch gentoo 推荐 efi 挂载在 /efi
        chroot $os_dir grub-install --efi-directory=/efi
        chroot $os_dir grub-install --efi-directory=/efi --removable
    else
        chroot $os_dir grub-install /dev/$xda
    fi

    # cmdline + 生成 grub.cfg
    if [ -d $os_dir/etc/default/grub.d ]; then
        file=$os_dir/etc/default/grub.d/tty.cfg
    else
        file=$os_dir/etc/default/grub
    fi
    ttys_cmdline=$(get_ttys console=)
    echo GRUB_CMDLINE_LINUX=\"\$GRUB_CMDLINE_LINUX $ttys_cmdline\" >>$file
    chroot $os_dir grub-mkconfig -o /boot/grub/grub.cfg

    # fstab
    # fstab 可不写 efi 条目， systemd automount 会自动挂载
    # fstab 头部有使用说明，因此用 >>
    local alpine_rootfs=$os_dir/alpine
    create_alpine_rootfs_with_arch_install_scripts "$alpine_rootfs" true "$os_dir"
    # genfstab 会用到 findmnt 等工具
    retry 5 chroot "$alpine_rootfs" apk add util-linux
    chroot "$alpine_rootfs" genfstab -U /parent | sed '/swap/d' >>$os_dir/etc/fstab
    umount -R "$alpine_rootfs/parent"
    remove_alpine_rootfs "$alpine_rootfs"

    # 删除 resolv.conf，不然 systemd-resolved 无法创建软链接
    rm_resolv_conf $os_dir

}

aria2c() {
    if ! is_have_cmd aria2c; then
        apk add aria2
    fi

    # stdbuf 在 coreutils 包里面
    if ! is_have_cmd stdbuf; then
        apk add coreutils
    fi

    # 显示 url
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
    # alpine 本身关闭了 64bit ext4
    # https://gitlab.alpinelinux.org/alpine/alpine-conf/-/blob/3.18.1/setup-disk.in?ref_type=tags#L908
    # 而且 alpine 的 extlinux 不兼容 64bit ext4
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

    # 防止文件未创建
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

            # 旧版 cloud-init 有 bug
            # 有的版本会只从第一种配置中读取 dns，有的从第二种读取
            # 因此写两种配置
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
        # 无法设置 autoconf = false ?
        if should_disable_accept_ra; then
            yq -i ".network.config[$config_id].accept-ra = false" $ci_file
        fi

        # 有 ipv6 但需设置 dns 的情况
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
        # 如果 network.config[$config_id] 没有 address，则删除，避免低版本 cloud-init 报错
        yq -i "del(.network.config[$config_id] | select(has(\"address\") | not))" $ci_file
    fi

    apk del yq-go

    # 查看文件
    info "Cloud-init network config"
    cat -n $ci_file >&2
}

# 实测没用，生成的 machine-id 是固定的
# 而且 lightsail centos 9 模板 machine-id 也是相同的，显然相同 id 不是个问题
clear_machine_id() {
    local os_dir=$1

    # https://www.freedesktop.org/software/systemd/man/latest/machine-id.html
    # gentoo 不会自动创建该文件
    echo uninitialized >$os_dir/etc/machine-id

    # https://build.opensuse.org/projects/Virtualization:Appliances:Images:openSUSE-Leap-15.5/packages/kiwi-templates-Minimal/files/config.sh?expand=1
    rm -f $os_dir/var/lib/systemd/random-seed
}

is_file_or_link() {
    # -e / -f 坏软连接，返回 false
    # -L 坏软连接，返回 true
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

# 抄 https://github.com/alpinelinux/alpine-conf/blob/3.18.1/setup-disk.in#L421
get_alpine_firmware_pkgs() {
    # 需要有 modloop，不然 modinfo 会报错
    ensure_service_started modloop >&2

    # 如果不在单独的文件夹，则用 linux-firmware-other
    # 如果在单独的文件夹，则用 linux-firmware-xxx
    # 如果不需要 firmware，则用 linux-firmware-none
    firmware_pkgs=$(
        cd /sys/module && modinfo -F firmware -- * 2>/dev/null |
            awk -F/ '{print $1 == $0 ? "linux-firmware-other" : "linux-firmware-"$1}' |
            sort -u
    )

    # 使用 command 因为自己覆盖了 apk 添加了 >&2
    retry 5 command apk search --quiet --exact ${firmware_pkgs:-linux-firmware-none}
}

get_ucode_firmware_pkgs() {
    is_virt && return

    case "$distro" in
    *) os=$distro ;;
    esac

    case "$os-$(get_cpu_vendor)" in
    # alpine 的 linux-firmware 以文件夹进行拆分
    # setup-alpine 会自动安装需要的 firmware（modloop 没挂载则无效）
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

    # 锁定用户后 ssh 能否登录
    # alpine ×
    # 其它系统 √

    # root 空密码，不锁定 root，其它用户用 su - root 能否切换到 root
    # alpine ×
    # 其它系统 √

    # centos 7 不支持一行命令同时 -d -l
    # passwd: Only one of -l, -u, -d, -S may be specified.

    # 删除密码
    chroot "$os_dir" passwd -d "$username"

    # 锁定用户
    if ! [ -e "$os_dir/etc/alpine-release" ]; then
        chroot "$os_dir" passwd -l "$username"
    fi

    # alpine 锁定用户无法登录 ssh
    # 因为 alpine 默认不开启 pam
    # 其他系统默认开启

    # 不开启 pam 的话，锁定用户无法登录 ssh
    # 开启 pam 后可以

    # alpine 是通过安装 openssh-server-pam 开启 pam
    # 不需要设置 UsePAM yes 也无法识别 UsePAM yes
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

    # 添加公钥
    if true; then
        (
            umask 077
            mkdir -p "$os_dir/$user_home/.ssh"
            cat /configs/ssh_keys >"$os_dir/$user_home/.ssh/authorized_keys"
        )
        # 注意要用 chroot，否则 uid/gid 是 alpine live os 下的 uid/gid
        chroot "$os_dir" chown "$username:$username" "$user_home"
        chroot "$os_dir" chown "$username:$username" "$user_home/.ssh"
        chroot "$os_dir" chown "$username:$username" "$user_home/.ssh/authorized_keys"
    else
        (
            # 如果日后添加 bsd 无法 chroot 时可以这样
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

    # 删除密码/锁定用户
    del_user_password_and_lock "$os_dir" "$username"

    # debian 云镜像 /etc/shadow 的 root 条目为
    # root:!unprovisioned:20591:0:99999:7:::
    # 首次开机会停在设置 root 密码界面，且阻塞 ssh 服务
    # 因此这里手动清空 root 密码并锁定
    if ! [ "$username" = root ] && is_have_cmd_on_disk "$os_dir" systemd-firstboot; then
        del_user_password_and_lock "$os_dir" root
    fi
}

is_ssh_kv_effective() {
    local os_dir=$1
    local key=$2
    local value=$3

    # 解决 ubuntu 22.04 报错
    # Missing privilege separation directory: /run/sshd
    if [ -d "$os_dir/run/sshd" ]; then
        we_create_run_sshd_dir=false
    else
        we_create_run_sshd_dir=true
        mkdir -p "$os_dir/run/sshd"
    fi

    # centos 7 / ubuntu 22.04 不支持 -G
    # -G 只检测配置文件
    # -T 会检测配置文件、host key
    if res=$(chroot "$os_dir" sshd -G 2>/dev/null || chroot "$os_dir" sshd -T 2>/dev/null); then
        # 删除自己创建的，避免后续权限不准确
        if $we_create_run_sshd_dir; then
            rm -rf "$os_dir/run/sshd"
        fi

        # centos 7 设置 prohibit-password ，sshd -T 会显示成 without-password
        printf "%s\n" "$res" |
            sed 's/^permitrootlogin without-password$/permitrootlogin prohibit-password/i' |
            if [ -n "$value" ]; then
                grep -F -xiq "$key $value"
            else
                # value 为空时，只验证 key 是否存在
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
    local explicit=${4:-false} # 是否需要显式设置

    # 有些发行版自带了某些配置，例如
    # ubuntu:
    # cat /etc/ssh/sshd_config.d/60-cloudimg-settings.conf | grep -i PasswordAuthentication
    # PasswordAuthentication no

    # gentoo:
    # cat /etc/ssh/sshd_config.d/9999999gentoo-pam.conf | grep -i PasswordAuthentication
    # PasswordAuthentication no

    # 0. 如果已经有这个配置，且不需要显式设置，则不修改
    if is_ssh_kv_effective "$os_dir" "$key" "$value" && ! $explicit; then
        return
    fi

    if line="^$key .*" && grep -Exiq "$line" $os_dir/etc/ssh/sshd_config 2>/dev/null; then
        # 1. 如果 sshd_config 存在此 key（非注释状态），则替换
        sed -Ei "s/$line/$key $value/" $os_dir/etc/ssh/sshd_config
    elif include_line='^Include .*/etc/ssh/sshd_config.d' &&
        # 2. 如果 sshd_config 设置了读取 sshd_config.d
        #    则写入到 sshd_config.d/01-xxx.conf

        # arch 没有 /etc/ssh/sshd_config.d/ 文件夹
        # opensuse tumbleweed 没有 /etc/ssh/sshd_config
        #                       有 /etc/ssh/sshd_config.d/ 文件夹
        #                       有 /usr/etc/ssh/sshd_config
        { grep -iq "$include_line" $os_dir/etc/ssh/sshd_config ||
            grep -iq "$include_line" $os_dir/usr/etc/ssh/sshd_config; } 2>/dev/null; then
        mkdir -p $os_dir/etc/ssh/sshd_config.d/
        echo "$key $value" >"$os_dir/etc/ssh/sshd_config.d/01-$(echo "$key" | to_lower).conf"
    else
        # 3. 写入 sshd_config
        #    如果 sshd_config 存在此 key (无论是否已注释)，则替换，包括删除注释
        #    否则追加
        line="^[# ]*$key .*"
        if grep -Exiq "$line" $os_dir/etc/ssh/sshd_config; then
            sed -Ei "s/$line/$key $value/" $os_dir/etc/ssh/sshd_config
        else
            echo "$key $value" >>$os_dir/etc/ssh/sshd_config
        fi
    fi

    # 验证是否成功
    if ! is_ssh_kv_effective "$os_dir" "$key" "$value"; then
        error_and_exit "Failed to set sshd config $key $value."
    fi
}

change_ssh_conf_for_key_login() {
    local os_dir=$1

    change_ssh_conf_if_different "$os_dir" PasswordAuthentication no

    # centos 7 PermitRootLogin 默认是 yes，而不是 prohibit-password
    if [ "$username" = root ]; then
        change_ssh_conf_if_different "$os_dir" PermitRootLogin prohibit-password
    fi

    # sshd -G/-T 有 ChallengeResponseAuthentication 说明是旧版 sshd
    # 才需要设置 ChallengeResponseAuthentication no

    # OpenSSH 8.6 和以下 (包括 el8/debian 11/ubuntu 20.04 等)
    # KbdInteractiveAuthentication ChallengeResponseAuthentication 可设置成不同的值
    # 如果没有显式设置 ChallengeResponseAuthentication no
    # 则 KbdInteractiveAuthentication no 不会生效 (sshd -G/-T 显示 KbdInteractiveAuthentication yes)

    # 因此先 sshd -G/-T 检测有没有 ChallengeResponseAuthentication 这个 key
    # 只要有就显式设置为 no
    # 而且要先设置 ChallengeResponseAuthentication 后设置 KbdInteractiveAuthentication
    # 否则 change_ssh_conf_if_different 设置 KbdInteractiveAuthentication no 时会检测到不生效而报错

    # 用户传进来的 rhel-like 系统可能是支持 ChallengeResponseAuthentication 的旧版本
    # 因此即使 el8/debian 11/ubuntu 20.04 都 EOL 后也不能删除这里
    if is_ssh_kv_effective "$os_dir" ChallengeResponseAuthentication; then
        change_ssh_conf_if_different "$os_dir" ChallengeResponseAuthentication no true
    fi

    # PasswordAuthentication no
    # KbdInteractiveAuthentication yes (默认是 yes)
    # 这种情况可以用密码登录
    # ssh -o PreferredAuthentications=keyboard-interactive user@ip

    # 多数发行版都会在 sshd_config 里设置成 no
    # 但 opensuse 16 没有
    change_ssh_conf_if_different "$os_dir" KbdInteractiveAuthentication no
}

change_ssh_conf_for_password_login() {
    local os_dir=$1

    # opensuse 16/tumbleweed 安装 openssh-server-config-rootlogin
    # 会生成 /usr/etc/ssh/sshd_config.d/50-permit-root-login.conf
    # 但是如果用户删除了此文件，包有更新的话，可能会重新创建这个文件？
    # 因此先不用这个方法

    # PasswordAuthentication 默认是 yes
    # 但某些发行版会在 sshd_config.d 里设置 PasswordAuthentication no
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

# 暂时用不着
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

    # 添加用户
    if ! grep -q "^$username:" "$os_dir/etc/passwd"; then
        # debian 推荐使用 adduser 而不是 useradd
        # https://manpages.debian.org/trixie/passwd/useradd.8.en.html
        # useradd is a low level utility for adding users.
        # On Debian, administrators should usually use adduser(8) instead.

        # adduser 会从 /etc/adduser.conf 读取默认要添加的组
        # 然而通常这个值是空白

        # alpine
        if is_have_cmd_on_disk "$os_dir" adduser &&
            chroot "$os_dir" adduser --help 2>&1 | grep -Fq -- BusyBox; then
            chroot "$os_dir" adduser --disabled-password "$username"

        # 新版 debian/ubuntu
        elif is_have_cmd_on_disk "$os_dir" adduser &&
            chroot "$os_dir" adduser --help 2>&1 | grep -Fq -- '--disabled-password' &&
            chroot "$os_dir" adduser --help 2>&1 | grep -Fq -- '--comment'; then
            chroot "$os_dir" adduser --disabled-password --comment '' "$username"

        # 旧版 debian/ubuntu
        elif is_have_cmd_on_disk "$os_dir" adduser &&
            chroot "$os_dir" adduser --help 2>&1 | grep -Fq -- '--disabled-password' &&
            chroot "$os_dir" adduser --help 2>&1 | grep -Fq -- '--gecos'; then
            chroot "$os_dir" adduser --disabled-password --gecos '' "$username"

        # el
        elif is_have_cmd_on_disk "$os_dir" adduser &&
            chroot "$os_dir" adduser --help 2>&1 | grep -Fq -- '--password'; then
            chroot "$os_dir" adduser --password ! "$username"

        # arch/gentoo 默认没有 adduser
        else
            chroot "$os_dir" useradd -m "$username"
        fi
    fi

    # 添加到 wheel/sudo 组
    if ! [ "$username" = root ]; then
        if [ -e "$os_dir/etc/alpine-release" ]; then
            # alpine
            # https://github.com/alpinelinux/alpine-conf/blob/master/setup-user.in#L168

            # 安装 doas
            chroot "$os_dir" apk add doas doas-sudo-shim
            mkdir -p "$os_dir/etc/doas.d"

            # 添加用户到组
            chroot "$os_dir" addgroup "$username" wheel

            # doas: 添加 wheel 组
            local file="$os_dir/etc/doas.d/20-wheel.conf"
            local content="permit persist :wheel"
            if ! grep -q "^$content" "$file" 2>/dev/null; then
                echo "$content" >>"$file"
            fi

            # doas: 添加单个用户 nopass
            echo "permit nopass $username" >"$os_dir/etc/doas.d/99-$username.conf"
        else
            # 通常用 wheel 组
            # debian/ubuntu 没有 wheel 组，只有 sudo 组

            # aws lightsail 上测试默认用户加入了哪些组
            # debian       admin : admin adm dialout cdrom floppy sudo audio dip video plugdev
            # ubuntu       ubuntu : ubuntu adm cdrom sudo dip lxd
            # almalinux    ec2-user : ec2-user adm systemd-journal
            # opensuse     ec2-user : ec2-user

            # 添加用户到组
            for group in \
                wheel sudo \
                adm dialout cdrom floppy audio dip video plugdev lxd systemd-journal; do
                if grep -q "^$group:" "$os_dir/etc/group"; then
                    # chroot "$os_dir" addgroup "$username" "$group"
                    chroot "$os_dir" usermod -aG "$group" "$username"
                fi
            done

            # sudo: gentoo 安装 sudo 后也没有 /etc/sudoers.d
            if ! [ -d "$os_dir/etc/sudoers.d" ]; then
                install -d -m 0750 "$os_dir/etc/sudoers.d"
            fi

            # sudo: 添加单个用户 NOPASSWD
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

            # 通过 /etc/pam.d/chpasswd 找到 /etc/pam.d/system-auth 或者 /etc/pam.d/system-auth
            # 再找到有 password 和 pam_unix.so 的行，并删除 use_authtok，写入 /etc/pam.d/chpasswd
            files=$(grep -E '^(password|@include)' $pam_d/chpasswd | awk '{print $NF}' | sort -u)
            for file in $files; do
                if [ -f "$pam_d/$file" ] && line=$(grep ^password "$pam_d/$file" | grep -F pam_unix.so); then
                    echo "$line" | sed 's/use_authtok//' >$pam_d/chpasswd
                    break
                fi
            done
        fi

        # 分两行写，不然遇到错误不会终止
        plaintext=$(get_password_plaintext)
        printf '%s\n' "$username:$plaintext" | chroot $os_dir chpasswd

        if $has_pamd_chpasswd; then
            mv $pam_d/chpasswd.orig $pam_d/chpasswd
        fi
    else
        printf '%s\n' "$username:$(get_password_linux_sha512)" | chroot $os_dir chpasswd -e
    fi
}

# 适用于 DD 后修复 gpt 备份分区表

mount_part_basic_layout() {
    local os_dir=$1
    local efi_dir=$2

    if is_efi || is_xda_gt_2t; then
        os_part_num=2
    else
        os_part_num=1
    fi

    # 挂载系统分区
    mkdir -p $os_dir
    mount -t ext4 "/dev/$(xda $os_part_num)" $os_dir

    # 挂载 efi 分区
    if is_efi; then
        mkdir -p $efi_dir
        mount -t vfat -o umask=077 "/dev/$(xda 1)" $efi_dir
    fi
}

# virt-what 要用最新版
# vultr 1G High Frequency LAX 实际上是 kvm
# debian 11 virt-what 1.19 显示为 hyperv qemu
# debian 11 systemd-detect-virt 显示为 microsoft
# alpine virt-what 1.25 显示为 kvm
# 所以不要在原系统上判断具体虚拟化环境

# lscpu 也可查看虚拟化环境，但 alpine on lightsail 运行结果为 Microsoft
# 猜测 lscpu 只参考了 cpuid 没参考 dmi
# virt-what 可能会输出多行结果，因此用 grep

# 将 AC/SAC 版本号 转换为 LTSC 版本号
# 用于查找驱动

# reinstall.sh 有同名方法

mkdir_clear() {
    local dir=$1

    if [ -z "$dir" ] || [ "$dir" = / ]; then
        return
    fi

    rm -rf "$dir"
    mkdir -p "$dir"
}

# 注意使用方法是 list=$(list_add "$list" "$item_to_add")

# reinstall.sh 有同名方法

# 添加 netboot.efi 备用

sync_time() {

    # ntp 时间差太多会无法同步？
    # http 时间可能不准确，毕竟不是专门的时间服务器
    #      也有可能没有 date header?
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
        # 可能有多行，取第一行
        date_header=$(wget -S --no-check-certificate --spider "$url" 2>&1 | grep -m1 '^  Date:')
        # gnu date 不支持 -D
        busybox date -u -D "  Date: %a, %d %b %Y %H:%M:%S GMT" -s "$date_header"
        ;;
    esac

    # 重启时 alpine 会自动写入到硬件时钟，因此这里跳过
    # hwclock -w
}

is_tencent_cloud() {
    [ "$(cat /sys/devices/virtual/dmi/id/sys_vendor 2>/dev/null)" = 'Tencent Cloud' ]
}

add_max_sectors_kb_rule() {
    local os_dir=$1

    # 普通发行版
    if [ -d $os_dir/etc/udev/rules.d/ ]; then
        # 取自腾讯云 ubuntu 26.04 镜像
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
        # 重新挂载为读写
        mount -o remount,rw /os

        # rule
        add_max_sectors_kb_rule "$os_dir"

        # 重新生成 initramfs
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

# 脚本入口

[ "$(uname -m)" = x86_64 ] || error_and_exit "Alpine Live installer requires x86_64."

# 复制脚本
# 用于打印错误或者再次运行
# 路径相同则不用复制
# 重点：要在删除脚本之前复制
if ! [ "$(readlink -f "$0")" = /trans.sh ]; then
    cp -f "$0" /trans.sh
fi
trap 'trap_err $LINENO $?' ERR

# 删除本脚本，不然会被复制到新系统
rm -f /etc/local.d/trans.start
rm -f /etc/runlevels/default/local

# 提取变量
extract_env_from_cmdline

# 带参数运行部分
# 重新下载并 exec 运行新脚本
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

# 无参数运行部分
# 允许 ramdisk 使用所有内存，默认是 50%
mount / -o remount,size=100%

# 同步时间
# 1. 可以防止访问 https 出错
# 2. 可以防止 https://github.com/bin456789/reinstall/issues/223
#    E: Release file for http://security.ubuntu.com/ubuntu/dists/noble-security/InRelease is not valid yet (invalid for another 5h 37min 18s).
#    Updates for this repository will not be applied.
# 4. 允许同步失败，因为不是关键步骤
sync_time || true

# 安装 ssh 并更改端口
apk add openssh-server
if is_need_change_ssh_port; then
    change_ssh_port / $ssh_port
fi

# 设置密码，添加开机启动 + 开启 ssh 服务
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

# 设置 frpc
# 并防止重复运行
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

# 正式运行重装
# shellcheck disable=SC2046,SC2194
case 1 in
1)
    # ChatGPT 说这种性能最高
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
