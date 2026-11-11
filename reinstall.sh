#!/usr/bin/env sh
# Personal Linux-only derivative of bin456789/reinstall; see docs/TRIMMING.md.
# shellcheck shell=bash
# shellcheck disable=SC2086

# nixos 默认的配置不会生成 /bin/bash，因此需要用 /usr/bin/env
# alpine 默认没有 bash，因此 shebang 用 sh，再 exec 切换到 bash

set -eE
confhome=${REINSTALL_CONFHOME:-}
# Optional network state; collect_netconf may leave this unset when no extra IPv6 address exists.
ipv6_extra_addrs=${ipv6_extra_addrs:-}

# 用于判断 reinstall.sh 和 trans.sh 是否兼容
SCRIPT_VERSION=PERSONAL-LINUX-ONLY-20261119-04

BOOT_ENTEY_START_MARK='### BEGIN reinstall.sh ###'
BOOT_ENTEY_END_MARK='### END reinstall.sh ###'

# 临时目录
# 不用 /tmp，因为 /tmp 挂载在内存的话，可能不够空间
tmp=/reinstall-tmp

# 强制 linux 程序输出英文，防止 grep 不到想要的内容
# https://www.gnu.org/software/gettext/manual/html_node/The-LANGUAGE-variable.html
export LC_ALL=C

# 处理部分用户用 su 切换成 root 导致环境变量没 sbin 目录
export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:$PATH

# 如果不是 bash 的话，继续执行会有语法错误，因此在这里判断是否 bash
if [ -z "$BASH" ] ||
    # el 的 sh 是 bash 运行在 posix 模式，依然有 $BASH 和 $BASH_VERSION
    { [ -n "$BASH" ] && [ -n "$POSIXLY_CORRECT" ]; }; then
    if ! command -v bash >/dev/null; then
        if [ -f /etc/alpine-release ]; then
            if ! apk add bash; then
                echo "Error while install bash." >&2
                exit 1
            fi
        else
            echo "Please run this script with bash." >&2
            exit 1
        fi
    fi
    exec bash "$0" "$@"
fi

# 好像跟 trap SIGINT 有冲突
# 记录日志，过滤含有 password 的行
# exec > >(tee >(grep -iv password >>/reinstall.log)) 2>&1
THIS_SCRIPT=$(readlink -f "$0")
trap 'trap_err $LINENO $?' ERR

trap_err() {
    line_no=$1
    ret_no=$2

    error "Line $line_no return $ret_no"
    sed -n "$line_no"p "$THIS_SCRIPT"
}

reinstall_____='bash reinstall.sh'

usage_and_exit() {
    cat <<EOF
Usage: $reinstall_____ alpine       (latest stable)
                       debian       (latest stable, installed from Alpine Live)
                       arch
                       gentoo
                       alpine --hold 1  (Alpine Live OS)
                       netboot.xyz
                       iso --iso /absolute/path/image.iso [--iso-config /boot/grub/loopback.cfg]
                       iso --iso /absolute/path/image.iso --iso-kernel /path/in/iso --iso-initrd /path/in/iso --iso-cmdline '...'
                       reset

Options: --username USERNAME --password PASSWORD --ssh-key KEY
         --ssh-port PORT --web-port PORT --frpc-config PATH
         --no-cloud-kernel --hold 0|1|2 --target-disk /dev/DISK
         --force-boot-mode bios|efi --confhome https://YOUR-ASSET-HOST/PATH
         --debug --help

Linux x86_64 hosts only. Automatic reinstallation ERASES the entire target disk.
Local ISO boot is interactive and requires loopback support; it does not format disks.
Assets must be hosted from THIS trimmed checkout; do not use the upstream asset base.
EOF
    exit "${1:-1}"
}

info() {
    local msg
    if [ "$1" = false ]; then
        shift
        msg=$*
    else
        msg="***** $(to_upper <<<"$*") *****"
    fi
    echo_color_text '\e[32m' "$msg" >&2
}

warn() {
    local msg
    if [ "$1" = false ]; then
        shift
        msg=$*
    else
        msg="Warning: $*"
    fi
    echo_color_text '\e[33m' "$msg" >&2
}

error() {
    echo_color_text '\e[31m' "***** ERROR *****" >&2
    echo_color_text '\e[31m' "$*" >&2
}

echo_color_text() {
    color="$1"
    shift
    plain="\e[0m"
    echo -e "$color$*$plain"
}

error_and_exit() {
    error "$@"
    exit 1
}

show_url_in_args() {
    while [ $# -gt 0 ]; do
        case "$1" in
        [Hh][Tt][Tt][Pp][Ss]://* | [Hh][Tt][Tt][Pp]://* | [Mm][Aa][Gg][Nn][Ee][Tt]:*) echo "$1" ;;
        esac
        shift
    done
}

curl() {
    is_have_cmd curl || install_pkg curl

    # 显示 url
    show_url_in_args "$@" >&2

    # 添加 -f, --fail，不然 404 退出码也为0
    # centos 7 curl 不支持 --retry-connrefused --retry-all-errors
    # 因此手动 retry
    for i in $(seq 5); do
        if command curl --connect-timeout 10 -f "$@"; then
            return
        else
            ret=$?
            # 403 404 错误，或者达到重试次数
            if [ $ret -eq 22 ] || [ $i -eq 5 ]; then
                return $ret
            fi
            sleep 1
        fi
    done
}

mask2cidr() {
    local x=${1##*255.}
    set -- 0^^^128^192^224^240^248^252^254^ $(((${#1} - ${#x}) * 2)) ${x%%.*}
    x=${1%%"$3"*}
    echo $(($2 + (${#x} / 4)))
}

is_in_alpine() {
    [ -f /etc/alpine-release ]
}

is_boot_in_separate_partition() {
    mount | grep -q ' on /boot type '
}

is_os_in_btrfs() {
    mount | grep -q ' on / type btrfs '
}

is_os_in_subvol() {
    subvol=$(awk '($2=="/") { print $i }' /proc/mounts | grep -o 'subvol=[^ ]*' | cut -d= -f2)
    [ "$subvol" != / ]
}

get_os_part() {
    awk '($2=="/") { print $1 }' /proc/mounts
}

umount_all() {
    if mount_lists=$(mount | grep -w "on $1" | awk '{print $3}' | grep .); then
        # alpine 没有 -R
        if umount --help 2>&1 | grep -wq -- '-R'; then
            umount -R "$1"
        else
            echo "$mount_lists" | tac | xargs -n1 umount
        fi
    fi
}

cp_to_btrfs_root() {
    mount_dir=$tmp/reinstall-btrfs-root
    if ! grep -q $mount_dir /proc/mounts; then
        mkdir -p $mount_dir
        mount "$(get_os_part)" $mount_dir -t btrfs -o subvol=/
    fi
    cp -rf "$@" "$mount_dir"
}

is_host_has_ipv4_and_ipv6() {
    host=$1

    install_pkg dig
    # dig会显示cname结果，cname结果以.结尾，grep -v '\.$' 用于去除 cname 结果
    res=$(dig +short $host A $host AAAA | grep -v '\.$')
    # 有.表示有ipv4地址，有:表示有ipv6地址
    grep -q \. <<<$res && grep -q : <<<$res
}

is_netboot_xyz() {
    [ "$distro" = netboot.xyz ]
}

is_alpine_live() {
    [ "$distro" = alpine ] && [ "$hold" = 1 ]
}

is_have_initrd() {
    ! is_netboot_xyz
}

is_digit() {
    [[ "$1" =~ ^[0-9]+$ ]]
}

is_port_valid() {
    is_digit "$1" && [ "$1" -ge 1 ] && [ "$1" -le 65535 ]
}

get_host_by_url() {
    cut -d/ -f3 <<<$1
}

get_scheme_and_host_by_url() {
    cut -d/ -f1-3 <<<$1
}

get_function() {
    declare -f "$1"
}

get_function_content() {
    declare -f "$1" | sed '1d;2d;$d'
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
}

test_url() {
    test_url_real false "$@"
}

test_url_grace() {
    test_url_real true "$@"
}

test_url_real() {
    grace=$1
    url=$2
    expect_types=$3
    var_to_eval=$4
    info test url

    failed() {
        $grace && return 1
        error_and_exit "$@"
    }

    tmp_file=$tmp/img-test

    # TODO: 好像无法识别 nixos 官方源的跳转
    # 有的服务器不支持 range，curl会下载整个文件
    # 所以用 head 限制 1M
    # 过滤 curl 23 错误（head 限制了大小）
    # ${PIPESTATUS[n]} 表示第n个管道的返回值
    echo $url
    for i in $(seq 5 -1 0); do
        if command curl --insecure --connect-timeout 10 -Lfr 0-1048575 "$url" \
            1> >(exec head -c 1048576 >$tmp_file) \
            2> >(exec grep -v 'curl: (23)' >&2); then
            break
        else
            ret=$?
            msg="$url not accessible"
            case $ret in
            22)
                # 403 404
                # 这里的 failed 虽然返回 1，但是不会中断脚本，因此要手动 return
                failed "$msg"
                return "$ret"
                ;;
            23)
                # 限制了空间
                break
                ;;
            *)
                # 其他错误
                if [ $i -eq 0 ]; then
                    failed "$msg"
                    return "$ret"
                fi
                ;;
            esac
            sleep 1
        fi
    done

    # 如果要检查文件类型
    if [ -n "$expect_types" ]; then
        install_pkg file
        real_type=$(file_enhanced $tmp_file)
        echo "File type: $real_type"

        # debian 9 ubuntu 16.04-20.04 可能会将 iso 识别成 raw
        for type in $expect_types $([ "$expect_types" = iso ] && echo raw); do
            if [[ ."$real_type" = *."$type" ]]; then
                # 如果要设置变量
                if [ -n "$var_to_eval" ]; then
                    IFS=. read -r "${var_to_eval?}" "${var_to_eval}_warp" <<<"$real_type"
                fi
                return
            fi
        done

        failed "$url
Expected type: $expect_types
Actually type: $real_type"
    fi
}

fix_file_type() {
    # gzip的mime有很多种写法
    # centos7中显示为 x-gzip，在其他系统中显示为 gzip，可能还有其他
    # 所以不用mime判断
    # https://www.digipres.org/formats/sources/tika/formats/#application/gzip

    # centos 7 上的 file 显示 qcow2 的 mime 为 application/octet-stream
    # file debian-12-genericcloud-amd64.qcow2
    # debian-12-genericcloud-amd64.qcow2: QEMU QCOW Image (v3), 2147483648 bytes
    # file --mime debian-12-genericcloud-amd64.qcow2
    # debian-12-genericcloud-amd64.qcow2: application/octet-stream; charset=binary

    # --extension 不靠谱
    # file -b /reinstall-tmp/img-test --mime-type
    # application/x-qemu-disk
    # file -b /reinstall-tmp/img-test --extension
    # ???

    # 1. 删除,;#
    # DOS/MBR boot sector; partition 1: ...
    # gzip compressed data, was ...
    # # ISO 9660 CD-ROM filesystem data... (有些 file 版本开头输出有井号)

    # 2. 删除开头的空格

    # 3. 删除无意义的单词 POSIX, Unicode, UTF-8, ASCII
    # POSIX tar archive (GNU)
    # Unicode text, UTF-8 text
    # UTF-8 Unicode text, with very long lines
    # ASCII text

    # 4. 下面两种都是 raw
    # DOS/MBR boot sector
    # x86 boot sector; partition 1: ...
    sed -E \
        -e 's/[,;#]//g' \
        -e 's/^[[:space:]]*//' \
        -e 's/(POSIX|Unicode|UTF-8|ASCII)//gi' \
        -e 's/^DOS\/MBR boot sector/raw/i' \
        -e 's/^x86 boot sector/raw/i' \
        -e 's/^Zstandard/zstd/i' \
        -e 's/^UDF/iso/i' \
        -e 's/^Windows imaging \(WIM\) image/wim/i' |
        awk '{print $1}' | to_lower
}

# 不用 file -z，因为
# 1. file -z 只能看透一层
# 2. alpine file -z 无法看透部分镜像（前1M），例如：
# guajibao-win10-ent-ltsc-2021-x64-cn-efi.vhd.gz
# guajibao-win7-sp1-ent-x64-cn-efi.vhd.gz
# win7-ent-sp1-x64-cn-efi.vhd.gz
# 还要注意 centos 7 没有 -Z 只有 -z
file_enhanced() {
    file=$1

    full_type=
    while true; do
        type="$(file -b $file | fix_file_type)"
        full_type="$type.$full_type"
        case "$type" in
        xz | gzip | zstd)
            install_pkg "$type"
            $type -dc <"$file" | head -c 1048576 >"$file.inside"
            mv -f "$file.inside" "$file"
            ;;
        tar)
            install_pkg "$type"
            # 隐藏 gzip: unexpected end of file 提醒
            tar xf "$file" -O 2>/dev/null | head -c 1048576 >"$file.inside"
            mv -f "$file.inside" "$file"
            ;;
        *)
            break
            ;;
        esac
    done
    # shellcheck disable=SC2001
    echo "$full_type" | sed 's/\.$//'
}

# trans.sh 有相同方法
add_community_repo_for_alpine() {
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

is_in_container() {
    { is_have_cmd systemd-detect-virt && systemd-detect-virt -qc; } ||
        [ -d /proc/vz ] ||
        { [ -f /proc/1/environ ] && grep -q container=lxc /proc/1/environ; }
}

# 使用 | del_br ，但返回 del_br 之前返回值

is_virt() {
    if [ -z "$_is_virt" ]; then
        # aws t4g debian 11
        # systemd-detect-virt: 为 none，即使装了dmidecode
        # virt-what: 未装 deidecode时结果为空，装了deidecode后结果为aws
        # 所以综合两个命令的结果来判断
        if is_have_cmd systemd-detect-virt && systemd-detect-virt -v; then
            _is_virt=true
        fi

        if [ -z "$_is_virt" ]; then
            # debian 安装 virt-what 不会自动安装 dmidecode，因此结果有误
            install_pkg dmidecode virt-what
            # virt-what 返回值始终是0，所以用是否有输出作为判断
            if [ -n "$(virt-what)" ]; then
                _is_virt=true
            fi
        fi

        if [ -z "$_is_virt" ]; then
            _is_virt=false
        fi
        echo "VM: $_is_virt"
    fi
    $_is_virt
}

get_http_log_url() {
    echo "http://IP$([ "${web_port:-80}" = 80 ] || echo :$web_port)$web_path"
}

# 判断语言字符是否合法，允许全名和缩写

set_var() {
    # eval 不安全

    # 仅 bash 可用
    printf -v "$1" "%s" "$2"

    # 或者
    # IFS= read -r "$1" <<<"$2"
}

setos() {
    local step=$1
    local distro=$2
    local releasever=$3
    info set $step $distro $releasever

    set_osvar() {
        set_var "${step}_$1" "$2"
    }

    setos_netboot.xyz() {
        if is_efi; then
            set_osvar efi https://boot.netboot.xyz/ipxe/netboot.xyz.efi
        else
            set_osvar vmlinuz https://boot.netboot.xyz/ipxe/netboot.xyz.lkrn
        fi
    }

    setos_alpine() {
        is_virt && flavour=virt || flavour=lts

        # Use HTTP because Alpine Live synchronizes its clock after the initramfs has started.
        mirror=http://dl-cdn.alpinelinux.org/alpine/v$releasever
        set_osvar vmlinuz "$mirror/releases/$basearch/netboot/vmlinuz-$flavour"
        set_osvar initrd "$mirror/releases/$basearch/netboot/initramfs-$flavour"
        set_osvar modloop "$mirror/releases/$basearch/netboot/modloop-$flavour"
        set_osvar repo "$mirror/main"
    }

    setos_debian() {
        codename=$latest_codename
        [[ "$codename" =~ ^[a-z]+$ ]] || error_and_exit "Debian codename was not resolved."
        set_osvar codename "$codename"
        set_osvar mirror https://deb.debian.org/debian
    }

    setos_arch() {
        mirror=https://geo.mirror.pkgbuild.com # geoip
        test_url $mirror/core/os/x86_64/core.db gzip
        set_osvar mirror "$mirror"
    }

    setos_gentoo() {
        mirror=https://distfiles.gentoo.org # cdn77

        dir=releases/$basearch_alt/autobuilds

        if [ -z "$img" ]; then
            prefix=stage3-$basearch_alt-systemd
            filename=$(curl -L $mirror/$dir/latest-$prefix.txt | grep '.tar.xz' | awk '{print $1}' | grep .)
            img=$mirror/$dir/$filename
            test_url "$img" 'tar.xz'
        fi
        set_osvar img "$img"
    }

    # shellcheck disable=SC2154

    set_osvar distro "$distro"
    set_osvar releasever "$releasever"

    case "$distro" in
    alpine | debian | arch | gentoo | netboot.xyz) setos_$distro ;;
    *) error_and_exit "Unsupported internal target: $distro" ;;
    esac

    # 集中测试云镜像格式
}

resolve_latest_stable() {
    local metadata version codename
    case "$1" in
    alpine)
        metadata=$(curl -L "https://dl-cdn.alpinelinux.org/alpine/latest-stable/releases/$basearch/latest-releases.yaml") ||
            error_and_exit "Cannot resolve latest stable Alpine."
        version=$(printf '%s\n' "$metadata" | sed -nE 's/^[[:space:]]*version: *([0-9]+\.[0-9]+)\.[0-9]+$/\1/p' | sort -u)
        [[ "$version" =~ ^[0-9]+\.[0-9]+$ ]] || error_and_exit "Invalid Alpine release metadata."
        latest_releasever=$version
        ;;
    debian)
        metadata=$(curl -L https://deb.debian.org/debian/dists/stable/Release) ||
            error_and_exit "Cannot resolve latest stable Debian."
        version=$(printf '%s\n' "$metadata" | sed -nE 's/^Version: ([0-9]+)(\.[0-9]+)*$/\1/p')
        codename=$(printf '%s\n' "$metadata" | sed -nE 's/^Codename: ([a-z]+)$/\1/p')
        [[ "$version" =~ ^[0-9]+$ ]] && [[ "$codename" =~ ^[a-z]+$ ]] ||
            error_and_exit "Invalid Debian release metadata."
        latest_releasever=$version
        latest_codename=$codename
        ;;
    *) error_and_exit "Cannot resolve stable release for $1." ;;
    esac
}

get_latest_distro_releasever() {
    resolve_latest_stable "$1"
    printf '%s\n' "$latest_releasever"
}

# 检查是否为正确的系统名
verify_os_name() {
    local input
    input=$(printf '%s' "$*" | to_lower)
    releasever=
    case "$input" in
    alpine | debian)
        distro=$input
        resolve_latest_stable "$distro"
        releasever=$latest_releasever
        ;;
    arch | gentoo | netboot.xyz | iso | reset) distro=$input ;;
    *)
        error "Supported targets: alpine, debian, arch, gentoo, iso, netboot.xyz, reset (no version argument)."
        usage_and_exit
        ;;
    esac
}

verify_os_args() {
    if [ -n "$password" ] && [ -n "$ssh_keys" ]; then
        error_and_exit "Cannot set both password and ssh key."
    fi
    if [ "$distro" = netboot.xyz ] || [ "$distro" = iso ]; then
        [ -z "$username$password$ssh_keys$frpc_config$ssh_port$web_port$hold$xda$no_cloud_kernel" ] ||
            error_and_exit "SSH, frpc, ports and hold options are not supported for $distro."
    fi
    if [ "$distro" = iso ]; then
        validate_iso_args
    else
        [ -z "$iso$iso_kernel$iso_initrd$iso_config$iso_cmdline" ] || error_and_exit "ISO options require the iso target."
    fi
}

get_cmd_path() {
    # arch 云镜像不带 which
    # command -v 包括脚本里面的方法
    # ash 无效
    type -f -p $1
}

is_have_cmd() {
    get_cmd_path $1 >/dev/null 2>&1
}

install_pkg() {

    find_pkg_mgr() {
        [ -n "$pkg_mgr" ] && return

        # 查找方法1: 通过 ID / ID_LIKE
        # 因为可能装了多种包管理器
        if [ -f /etc/os-release ]; then
            # shellcheck source=/dev/null
            for id in $({ . /etc/os-release && echo $ID $ID_LIKE; }); do
                # https://github.com/chef/os_release
                case "$id" in
                fedora | centos | rhel) is_have_cmd dnf && pkg_mgr=dnf || pkg_mgr=yum ;;
                debian | ubuntu) pkg_mgr=apt-get ;;
                opensuse | suse) pkg_mgr=zypper ;;
                alpine) pkg_mgr=apk ;;
                arch) pkg_mgr=pacman ;;
                gentoo) pkg_mgr=emerge ;;
                nixos) pkg_mgr=nix-env ;;
                esac
                [ -n "$pkg_mgr" ] && return
            done
        fi

        # 查找方法 2
        for mgr in dnf yum apt-get pacman zypper emerge apk nix-env; do
            is_have_cmd $mgr && pkg_mgr=$mgr && return
        done

        return 1
    }

    cmd_to_pkg() {
        unset USE
        case $cmd in
        ar)
            case "$pkg_mgr" in
            *) pkg="binutils" ;;
            esac
            ;;
        xz)
            case "$pkg_mgr" in
            apt-get) pkg="xz-utils" ;;
            *) pkg="xz" ;;
            esac
            ;;
        lsblk | findmnt)
            case "$pkg_mgr" in
            apk) pkg="$cmd" ;;
            *) pkg="util-linux" ;;
            esac
            ;;
        lsmem)
            case "$pkg_mgr" in
            apk) pkg="util-linux-misc" ;;
            *) pkg="util-linux" ;;
            esac
            ;;
        fdisk)
            case "$pkg_mgr" in
            apt-get) pkg="fdisk" ;;
            apk) pkg="util-linux-misc" ;;
            *) pkg="util-linux" ;;
            esac
            ;;
        hexdump)
            case "$pkg_mgr" in
            apt-get) pkg="bsdmainutils" ;;
            *) pkg="util-linux" ;;
            esac
            ;;
        unsquashfs)
            case "$pkg_mgr" in
            zypper) pkg="squashfs" ;;
            emerge) pkg="squashfs-tools" && export USE="lzma" ;;
            *) pkg="squashfs-tools" ;;
            esac
            ;;
        nslookup | dig)
            case "$pkg_mgr" in
            apt-get) pkg="dnsutils" ;;
            pacman) pkg="bind" ;;
            apk | emerge) pkg="bind-tools" ;;
            yum | dnf | zypper) pkg="bind-utils" ;;
            esac
            ;;
        iconv)
            case "$pkg_mgr" in
            apk) pkg="musl-utils" ;;
            *) error_and_exit "Which GNU/Linux do not have iconv built-in?" ;;
            esac
            ;;
        *) pkg=$cmd ;;
        esac
    }

    # 系统                       package名称                                    repo名称
    # centos/alma/rocky/fedora   epel-release                                   epel
    # oracle linux               oracle-epel-release                            ol9_developer_EPEL
    # opencloudos                epol-release                                   EPOL
    # alibaba cloud linux 3      epel-release/epel-aliyuncs-release(qcow2自带)  epel
    # anolis 23                  anolis-epao-release                            EPAO

    # anolis 8
    # [root@localhost ~]# yum search *ep*-release | grep -v next
    # ========================== Name Matched: *ep*-release ==========================
    # anolis-epao-release.noarch : EPAO Packages for Anolis OS 8 repository configuration
    # epel-aliyuncs-release.noarch : Extra Packages for Enterprise Linux repository configuration
    # epel-release.noarch : Extra Packages for Enterprise Linux repository configuration (qcow2自带)

    check_is_need_epel() {
        is_need_epel() {
            case "$pkg" in
            dpkg) true ;;
            jq) is_have_cmd yum && ! is_have_cmd dnf ;; # el7/ol7 的 jq 在 epel 仓库
            *) false ;;
            esac
        }

        get_epel_repo_name() {
            # el7 不支持 yum repolist --all，要使用 yum repolist all
            # el7 yum repolist 第一栏有 /x86_64 后缀，因此要去掉。而 el9 没有
            $pkg_mgr repolist all | awk '{print $1}' | awk -F/ '{print $1}' | grep -Ei 'ep(el|ol|ao)$'
        }

        get_epel_pkg_name() {
            # el7 不支持 yum list --available，要使用 yum list available
            $pkg_mgr list available | grep -E '(.*-)?ep(el|ol|ao)-(.*-)?release' |
                awk '{print $1}' | cut -d. -f1 | grep -v next | head -1
        }

        if is_need_epel; then
            if ! epel=$(get_epel_repo_name); then
                $pkg_mgr install -y "$(get_epel_pkg_name)"
                epel=$(get_epel_repo_name)
            fi
            enable_epel="--enablerepo=$epel"
        else
            enable_epel=
        fi
    }

    install_pkg_real() {
        text="$pkg"
        if [ "$pkg" != "$cmd" ]; then
            text+=" ($cmd)"
        fi
        echo "Installing package '$text'..."

        case $pkg_mgr in
        dnf)
            check_is_need_epel
            dnf install $enable_epel -y --setopt=install_weak_deps=False $pkg
            ;;
        yum)
            check_is_need_epel
            yum install $enable_epel -y $pkg
            ;;
        emerge) emerge --oneshot $pkg ;;
        pacman) pacman -Syu --noconfirm --needed $pkg ;;
        zypper) zypper install -y $pkg ;;
        apk)
            add_community_repo_for_alpine
            apk add $pkg
            ;;
        apt-get)
            [ -z "$apt_updated" ] && apt-get update && apt_updated=1
            DEBIAN_FRONTEND=noninteractive apt-get install -y $pkg
            ;;
        nix-env)
            # 不指定 channel 会很慢，而且很占内存
            [ -z "$nix_updated" ] && nix-channel --update && nix_updated=1
            nix-env -iA nixos.$pkg
            ;;
        esac
    }

    is_need_reinstall() {
        local cmd=$1

        # gentoo 默认编译的 unsquashfs 不支持 xz
        if [ "$cmd" = unsquashfs ] && is_have_cmd emerge && ! "$cmd" |& grep -wq xz; then
            echo "unsquashfs not supported xz. rebuilding."
            return 0
        fi

        # busybox grep  不支持 -oP
        # busybox lsblk 不支持 -r -n --inverse
        # busybox fdisk 无法显示 mbr 分区表的 id
        if { [ "$cmd" = grep ] || [ "$cmd" = lsblk ] || [ "$cmd" = fdisk ]; } &&
            is_have_cmd apk && "$cmd" --help |& grep -wq BusyBox; then
            return 0
        fi

        return 1
    }

    for cmd in "$@"; do
        if ! is_have_cmd $cmd || is_need_reinstall $cmd; then
            if ! find_pkg_mgr; then
                error_and_exit "Can't find compatible package manager. Please manually install $cmd."
            fi
            cmd_to_pkg
            install_pkg_real
        fi
    done >&2
}

is_valid_ram_size() {
    is_digit "$1" && [ "$1" -gt 0 ]
}

ram_floor_for_target() {
    case "$1" in
    alpine | debian) echo 512 ;;
    arch) echo 1024 ;;
    gentoo) echo 2048 ;;
    netboot.xyz) echo 0 ;;
    *) error_and_exit "Cannot determine RAM floor for $1." ;;
    esac
}

check_ram() {
    ram_standard=$(ram_floor_for_target "$distro")

    # 不用检查内存的情况
    if [ "$ram_standard" -eq 0 ]; then
        return
    fi

    # lsmem is preferred; keep dmidecode/lshw fallbacks for older source distributions.
    install_pkg lsmem
    ram_size=$(lsmem -b 2>/dev/null | grep 'Total online memory:' | awk '{ print $NF/1024/1024 }')

    if ! is_valid_ram_size "$ram_size"; then
        install_pkg dmidecode
        ram_size=$(dmidecode -t 17 | grep "Size.*[GM]B" | awk '{if ($3=="GB") s+=$2*1024; else s+=$2} END {if(s>0) print s}')
    fi

    if ! is_valid_ram_size "$ram_size"; then
        install_pkg lshw
        # 不能忽略 -i，alpine 显示的是 System memory
        ram_str=$(lshw -c memory -short | grep -i 'System Memory' | awk '{print $3}')
        ram_size=$(grep <<<$ram_str -o '[0-9]*')
        grep <<<$ram_str GiB && ram_size=$((ram_size * 1024))
    fi

    # 用于兜底，不太准确
    if ! is_valid_ram_size "$ram_size"; then
        ram_size_k=$(grep '^MemTotal:' /proc/meminfo | awk '{print $2}')
        ram_size=$((ram_size_k / 1024 + 64 + 4))
    fi

    if ! is_valid_ram_size "$ram_size"; then
        error_and_exit "Could not detect RAM size."
    fi

    # No cloud-image fallback in this four-target installation interface.
    if [ "$ram_size" -lt "$ram_standard" ]; then
        error_and_exit "Could not install $distro: RAM < $ram_standard MB."
    fi

}

is_efi() {
    [ -d /sys/firmware/efi ]
}

is_grub_dir_linked() {
    # cloudcone 重装前/重装后(方法1)
    [ "$(readlink -f /boot/grub/grub.cfg)" = /boot/grub2/grub.cfg ] ||
        [ "$(readlink -f /boot/grub2/grub.cfg)" = /boot/grub/grub.cfg ] ||
        # cloudcone 重装后(方法2)
        { [ -f /boot/grub2/grub.cfg ] && [ "$(cat /boot/grub2/grub.cfg)" = 'chainloader (hd0)+1' ]; }
}

is_secure_boot_enabled() {
    if is_efi; then
        if dmesg | grep -i 'Secure boot enabled'; then
            return 0
        fi
        install_pkg mokutil
        mokutil --sb-state 2>&1 | grep -i 'SecureBoot enabled'
    else
        return 1
    fi
}

is_need_boot_vmlinuz() {
    ! { is_netboot_xyz && is_efi; }
}

# 只有 linux bios 是用本机的 grub/extlinux
is_use_local_grub_extlinux() {
    is_need_boot_vmlinuz && ! is_efi
}

is_use_local_grub() {
    is_use_local_grub_extlinux && is_mbr_using_grub
}

is_use_local_extlinux() {
    is_use_local_grub_extlinux && ! is_mbr_using_grub
}

# 软 raid 时 xda 可能不是引导盘，以后再修正
is_mbr_using_grub() {
    find_main_disk
    # 各发行版不一定自带 strings hexdump xxd od 命令
    head -c 440 /dev/$xda | grep -a -iq 'GRUB'
}

to_upper() {
    tr '[:lower:]' '[:upper:]'
}

to_lower() {
    tr '[:upper:]' '[:lower:]'
}

del_cr() {
    sed -E 's/\r+$//'
}

del_empty_lines() {
    sed '/^[[:space:]]*$/d'
}

del_comment_lines() {
    sed '/^[[:space:]]*#/d'
}

trim() {
    # sed -E -e 's/^[[:space:]]+//' -e 's/[[:space:]]+$//'
    sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//'
}

assert_username_valid() {
    # 不能为 none [ ] / \ : | < > + = ; , ? * % @

    # 账号为空
    if [ -z "$username" ]; then
        error_and_exit "Username: Can not be empty."
    fi

    # 账号为 none
    if [ "$(to_lower <<<"$username")" = none ]; then
        error_and_exit "Username: Can not be 'none'."
    fi

    # 账号包含非法字符
    if grep -q '[][/\:|<>+=;,?*%@]' <<<"$username"; then
        error_and_exit "Username: Do not use any of the following characters: / \ [ ] : | < > + = ; , ? * % @"
    fi
}

# trans.sh 有同名方法

prompt_username() {
    info "prompt username"

    default_username=root

    warn false "Set username, leave blank to use $default_username"
    warn false "设置用户名，不填写则使用 $default_username"
    IFS= read -r -p "Username: " username
    username="$(printf "%s" "$username" | trim)"

    if [ -z "$username" ]; then
        username=$default_username
    fi
    assert_username_valid
}

prompt_password() {
    info "prompt password"
    warn false "Set password, leave blank to use a random password."
    warn false "设置密码，不填写则使用随机密码"
    while true; do
        IFS= read -r -p "Password: " password
        if [ -n "$password" ]; then
            IFS= read -r -p "Retype password: " password_confirm
            if [ "$password" = "$password_confirm" ]; then
                break
            else
                error "Passwords don't match. Try again."
            fi
        else
            # 特殊字符列表
            # 有的机器运行 centos 7 ，用 /dev/random 产生 16 位密码，开启了 rngd 也要 5 秒，关闭了 rngd 则长期阻塞
            chars=\''A-Za-z0-9~!@#$%^&*_=+`|(){}[]:;"<>,.?/-'
            password=$(tr -dc "$chars" </dev/urandom | head -c16)
            break
        fi
    done
}

save_password() {
    dir=$1

    # mkpasswd 有三个
    # expect 里的 mkpasswd 是用来生成随机密码的
    # whois 里的 mkpasswd 才是我们想要的，可能不支持 yescrypt，alpine 的 mkpasswd 是独立的包
    # busybox 里的 mkpasswd 也是我们想要的，但多数不支持 yescrypt

    # alpine 这两个包有冲突
    # apk add expect mkpasswd

    # 不要用 echo "$password" 保存密码，原因：
    # password="-n"
    # echo "$password"  # 空白

    # 明文密码
    # 假如用户运行 alpine live 直接打包硬盘镜像，如果保存了明文密码，则会暴露明文密码，因为 netboot initrd 在里面
    # 通过 --password 传入密码，history 有记录，也会暴露明文密码
    # /reinstall.log 也会暴露明文密码（已处理）

    # sha512
    # 以下系统均支持 sha512 密码，但是生成密码需要不同的工具
    # 兼容性     openssl   mkpasswd          busybox  python
    # centos 7     ×      只有expect的       需要编译    √
    # centos 8     √      只有expect的
    # debian 9     ×         √
    # ubuntu 16    ×         √
    # alpine       √      可能系统装了expect     √
    # others       √

    # alpine
    if is_have_cmd busybox && busybox mkpasswd --help 2>&1 | grep -wq sha512; then
        crypted=$(printf '%s' "$password" | busybox mkpasswd -m sha512)
    # others
    elif install_pkg openssl && openssl passwd --help 2>&1 | grep -wq '\-6'; then
        crypted=$(printf '%s' "$password" | openssl passwd -6 -stdin)
    # debian 9 / ubuntu 16
    elif is_have_cmd apt-get && install_pkg whois && mkpasswd -m help | grep -wq sha-512; then
        crypted=$(printf '%s' "$password" | mkpasswd -m sha-512 --stdin)
    # centos 7
    # crypt.mksalt 是 python3 的
    # 红帽把它 backport 到了 centos7 的 python2 上
    # 在其它发行版的 python2 上运行会出错
    elif is_have_cmd yum && is_have_cmd python2; then
        crypted=$(python2 -c "import crypt, sys; print(crypt.crypt(sys.argv[1], crypt.mksalt(crypt.METHOD_SHA512)))" "$password")
    else
        error_and_exit "Could not generate sha512 password."
    fi
    echo "$crypted" >"$dir/password-linux-sha512"

    # yescrypt
    # 旧系统不支持，先不管

}

# 记录主硬盘
find_main_disk() {
    if [ -n "$main_disk" ]; then
        return
    fi

    if [ -z "$xda" ]; then
        # centos7下测试     lsblk --inverse $mapper | grep -w disk     grub2-probe -t disk /
        # 跨硬盘btrfs       只显示第一个硬盘                            显示两个硬盘
        # 跨硬盘lvm         显示两个硬盘                                显示/dev/mapper/centos-root
        # 跨硬盘软raid      显示两个硬盘                                显示/dev/md127

        # 还有 findmnt

        # 理论上 /boot/efi /efi /boot 可以不在主硬盘上
        # 因此只查找 / 分区

        install_pkg lsblk
        # lvm 显示的是 /dev/mapper/xxx-yyy，再用第二条命令得到sda
        mapper=$(mount | awk '$3=="/" {print $1}' | grep .)
        xdas=$(lsblk -rn --inverse $mapper | grep -w disk | awk '{print $1}' | sort -u | grep .)

        # 注意 wc -l 的坑
        # wc -l <<<"" 输出 1

        # 检测主硬盘是否横跨多个磁盘
        if [ "$(wc -l <<<"$xdas")" -eq 1 ]; then
            xda=$xdas
        else
            # vultr 官网新建数据盘时有写，部分地区的实例，不能从数据盘启动
            # 实测 nvram 可写入数据盘的条目但不生效，手动安装 debian 到此硬盘，nvram 也不生效
            # 从 bios 手动选择数据盘的 efi 文件才能启动
            # 但是 nvram BootNext 数据盘的条目却可以生效

            # vultr 从软 raid 0 debian 下用本脚本安装 debian ，进入 grub efi 时，无法识别系统分区 md1 的文件系统，也少了 hd1
            # 但是从 bios 手动选择 reinstall 的 grub efi 时却可以识别
            # 后期应该将 vmlinux/initrd 放到 efi/boot 分区

            # Gemini 说:
            # 使用 efibootmgr 的 BootNext 时，固件走的是“快速路径”：
            # 它只初始化存放 EFI 引导文件（grubx64.efi）的那块硬盘（hd0），然后直接把控制权交给 GRUB。
            # 因为 hd1 还没“醒”，GRUB 自然无法拼凑出完整的 md1 阵列，导致无法识别文件系统。

            # 使用 nativedisk 可以强制 grub 识别所有硬盘?
            info false "Multiple disks found for root partition:"
            echo '-----'
            printf "%s\n" "$xdas"
            echo '-----'
            read -r -p "Select a disk to install: " xda
            if ! grep -Fqx "$xda" <<<"$xdas"; then
                error_and_exit "Invalid Input."
            fi
        fi
    fi

    info "Main disk: $xda"

    # 可以用 dd 找出 guid?

    # centos7 blkid lsblk 不显示 PTUUID
    # centos7 sfdisk 不显示 Disk identifier
    # alpine blkid 不显示 gpt 分区表的 PTUUID
    # 因此用 fdisk

    # Disk identifier: 0x36778223                                  # gnu fdisk + mbr
    # Disk identifier: D6B17C1A-FA1E-40A1-BDCB-0278A3ED9CFC        # gnu fdisk + gpt
    # Disk identifier (GUID): d6b17c1a-fa1e-40a1-bdcb-0278a3ed9cfc # busybox fdisk + gpt
    # 不显示 Disk identifier                                        # busybox fdisk + mbr

    # 获取 xda 的 id
    install_pkg fdisk
    main_disk=$(fdisk -l /dev/$xda | grep 'Disk identifier' | awk '{print $NF}' | sed 's/0x//')

    # 检查 id 格式是否正确
    if ! grep -Eix '[0-9a-f]{8}' <<<"$main_disk" &&
        ! grep -Eix '[0-9a-f-]{36}' <<<"$main_disk"; then
        error_and_exit "Disk ID is invalid: $main_disk"
    fi
}

is_found_ipv4_netconf() {
    [ -n "$ipv4_mac" ] && [ -n "$ipv4_addr" ] && [ -n "$ipv4_gateway" ]
}

is_found_ipv6_netconf() {
    [ -n "$ipv6_mac" ] && [ -n "$ipv6_addr" ] && [ -n "$ipv6_gateway" ]
}

# TODO: 单网卡多IP
collect_netconf() {
    # linux
    # 通过默认网关得到默认网卡

    # 多个默认路由下
    # ip -6 route show default dev ens3 完全不显示

    # ip -6 route show default
    # default proto static metric 1024 pref medium
    #         nexthop via 2a01:1111:262:4940::2 dev ens3 weight 1 onlink
    #         nexthop via fe80::5054:ff:fed4:5286 dev ens3 weight 1

    # ip -6 route show default
    # default via 2602:1111:0:80::1 dev eth0 metric 1024 onlink pref medium

    # arch + vultr
    # ip -6 route show default
    # default nhid 4011550343 via fe80::fc00:5ff:fe3d:2714 dev enp1s0 proto ra metric 1024 expires 1504sec pref medium

    for v in 4 6; do
        if via_gateway_dev_ethx=$(ip -$v route show default | grep -Ewo 'via [^ ]+ dev [^ ]+' | head -1 | grep .); then
            read -r _ gateway _ ethx <<<"$via_gateway_dev_ethx"
            set_var ipv${v}_ethx "$ethx"
            set_var ipv${v}_mac "$(ip link show dev $ethx | grep link/ether | head -1 | awk '{print $2}')"
            set_var ipv${v}_gateway "$gateway"

            # 获取所有全局地址
            all_addrs=$(ip -$v -o addr show scope global dev $ethx | grep -v temporary | awk '{print $4}')
            primary_addr=$(echo "$all_addrs" | head -1)

            # IPv6: 用 ip route get 让内核返回正确的源 IP，指定 dev 避免 tun/warp 干扰
            if [ "$v" = 6 ] && [ -n "$primary_addr" ]; then
                route_src=$(ip -6 route get 2001:4860:4860::8888 dev "$ethx" 2>/dev/null | grep -oP 'src \K[^ ]+')
                if [ -n "$route_src" ]; then
                    for addr in $all_addrs; do
                        if [ "${addr%/*}" = "$route_src" ]; then
                            primary_addr=$addr
                            break
                        fi
                    done
                fi
            fi

            set_var ipv${v}_addr "$primary_addr"
            # extra_addrs: 除主地址外的所有地址
            set_var ipv${v}_extra_addrs "$(echo "$all_addrs" | grep -Fxve "$primary_addr" | tr '\n' ',' | sed 's/,$//')"
        fi
    done

    if ! is_found_ipv4_netconf && ! is_found_ipv6_netconf; then
        error_and_exit "Can not get IP info."
    fi

    info "Network Info"
    echo "IPv4 MAC: $ipv4_mac"
    echo "IPv4 Address: $ipv4_addr"
    echo "IPv4 Gateway: $ipv4_gateway"
    echo "---"
    echo "IPv6 MAC: $ipv6_mac"
    echo "IPv6 Address: $ipv6_addr"
    echo "IPv6 Gateway: $ipv6_gateway"
    echo
}

get_maybe_efi_dirs_in_linux() {
    # 不从 fstab 查找，因为极端情况下可能只用 systemd mount
    # arch云镜像efi分区挂载在/efi，且使用 autofs，mount 命令会有两个 /efi 条目

    install_pkg findmnt >&2

    # 寻找 efi 分区，并输出根目录
    # root_dirs=$(mount | awk '$5=="vfat" || $5=="autofs" {print $3}' | grep -Ex '/efi|/boot/efi|/boot' | sort -u)
    root_dirs=$(findmnt -t fat,vfat -n -o TARGET | grep -Ex '/efi|/boot/efi|/boot' | sort -u)

    efi_dirs=$(
        for dir in $root_dirs; do
            # 只显示有 efi 文件的
            # -quit 表示找到第一个 *.efi 就立即退出 find
            if [ -d "$dir" ]; then
                find "$dir" -type f -iname "*.efi" -exec printf '%s\n' "$dir" \; -quit
            fi
        done
    )

    if [ -z "$efi_dirs" ]; then
        error_and_exit "Can't find efi partition."
    fi

    echo "$efi_dirs"
}

get_disk_by_part() {
    dev_part=$1
    install_pkg lsblk >&2
    lsblk -rn --inverse "$dev_part" | grep -w disk | awk '{print $1}'
}

get_part_num_by_part() {
    dev_part=$1
    grep -oE '[0-9]*$' <<<"$dev_part"
}

grep_efi_entry() {
    # efibootmgr
    # BootCurrent: 0002
    # Timeout: 1 seconds
    # BootOrder: 0000,0002,0003,0001
    # Boot0000* sles-secureboot
    # Boot0001* CD/DVD Rom
    # Boot0002* Hard Disk
    # Boot0003* sles-secureboot
    # MirroredPercentageAbove4G: 0.00
    # MirrorMemoryBelow4GB: false

    # 根据文档，* 表示 active，也就是说有可能没有*(代表inactive)
    # https://manpages.debian.org/testing/efibootmgr/efibootmgr.8.en.html
    grep -E '^Boot[0-9a-fA-F]{4}'
}

# trans.sh 有同名方法
grep_efi_index() {
    awk '{print $1}' | sed -e 's/Boot//' -e 's/\*//'
}

download_or_copy_file() {
    local source=$1
    local dist=$2

    mkdir -p "$(dirname $dist)"

    if [[ "$source" = http* ]]; then
        curl -Lo "$dist" "$source"
    else
        cp -f "$source" "$dist"
    fi
}

add_efi_entry_in_linux() {
    local source=$1

    info "Add efi entry in linux"

    install_pkg efibootmgr

    # 只取第一个
    # 因为不用关心是否为 efi 分区，只要是 fat/vfat 格式的分区即可添加到引导
    # 这里用了管道导致 get_maybe_efi_dirs_in_linux 里面的 error_and_exit 不生效
    efi_part=$(get_maybe_efi_dirs_in_linux | head -1 | grep .)
    dist_dir=$efi_part/EFI/reinstall
    basename=$(basename $source)
    download_or_copy_file "$source" "$dist_dir/$basename"

    # 原系统可能不是用 grub 引导，因此不一定有 grub-probe
    install_pkg findmnt
    # arch findmnt 会得到
    # systemd-1
    # /dev/sda2
    dev_part=$(findmnt -T "$dist_dir" -no SOURCE | grep '^/dev/')

    set -- efibootmgr --create-only \
        --disk "/dev/$(get_disk_by_part $dev_part)" \
        --part "$(get_part_num_by_part $dev_part)" \
        --label "$(get_entry_name)" \
        --loader "\\EFI\\reinstall\\$basename"

    if ! res=$("$@"); then
        echo "Command: $*"
        echo "$res"
        error_and_exit "Could not add efi entry."
    fi

    id=$(echo "$res" | grep_efi_entry | tail -1 | grep_efi_index | grep .)
    efibootmgr --bootnext "$id"
}

get_grub_efi_filename() {
    echo grubx64.efi
}

install_grub_linux_efi() {
    info 'download grub efi'
    grub_efi=$(get_grub_efi_filename)
    # Auxiliary GRUB binary, not a supported installation target.
    # Keep upstream's Fedora 44 GRUB source; do not derive it from target validation.
    local grub_release=44
    local mirror=https://d2lzkl7pfhq30w.cloudfront.net/pub/fedora/linux
    curl -Lo "$tmp/$grub_efi" "$mirror/releases/$grub_release/Everything/$basearch/os/EFI/BOOT/$grub_efi"
    add_efi_entry_in_linux "$tmp/$grub_efi"
}

download_and_extract_apk() {
    local alpine_ver=$1 package=$2 extract_dir=$3
    local mirror=https://dl-cdn.alpinelinux.org/alpine package_apk tar_args
    install_pkg tar xz
    package_apk=$(curl -L "$mirror/v$alpine_ver/main/$basearch/" | grep -oP "$package-[^-]*-[^-]*\\.apk" | sort -u)
    [ "$(wc -l <<<"$package_apk")" -eq 1 ] || error_and_exit "find no/multi apks."
    mkdir -p "$extract_dir"
    tar 2>&1 | grep -q BusyBox && tar_args= || tar_args=--warning=no-unknown-keyword
    curl -L "$mirror/v$alpine_ver/main/$basearch/$package_apk" | tar xz $tar_args -C "$extract_dir"
}

find_grub_extlinux_cfg() {
    dir=$1
    filename=$2
    keyword=$3

    # 当 ln -s /boot/grub /boot/grub2 时
    # find /boot/ 会自动忽略 /boot/grub2 里面的文件
    cfgs=$(
        # 只要 $dir 存在
        # 无论是否找到结果，返回值都是 0
        find $dir \
            -type f -name $filename \
            -exec grep -E -l "$keyword" {} \;
    )

    count="$(wc -l <<<"$cfgs")"
    if [ "$count" -eq 1 ]; then
        echo "$cfgs"
    else
        error_and_exit "Find $count $filename."
    fi
}

# 空格、&、用户输入的网址要加引号，否则 grub 无法正确识别
is_need_quote() {
    [[ "$1" = *' '* ]] || [[ "$1" = *'&'* ]] || [[ "$1" = http* ]]
}

# 转换 finalos_a=1 为 finalos.a=1 ，排除 finalos_mirrorlist
build_finalos_cmdline() {
    if vars=$(compgen -v finalos_); then
        for key in $vars; do
            value=${!key}
            key=${key#finalos_}
            if [ -n "$value" ] && [ $key != "mirrorlist" ]; then
                is_need_quote "$value" &&
                    finalos_cmdline+=" finalos_$key='$value'" ||
                    finalos_cmdline+=" finalos_$key=$value"
            fi
        done
    fi
}

build_extra_cmdline() {
    for key in confhome hold force_boot_mode no_cloud_kernel main_disk \
        username ssh_port web_port web_path; do
        value=${!key}
        if [ -n "$value" ]; then
            is_need_quote "$value" &&
                extra_cmdline+=" extra_$key='$value'" ||
                extra_cmdline+=" extra_$key=$value"
        fi
    done

    # 指定最终安装系统的 mirrorlist，链接有&，在grub中是特殊字符，所以要加引号
    if [ -n "$finalos_mirrorlist" ]; then
        extra_cmdline+=" extra_mirrorlist='$finalos_mirrorlist'"
    elif [ -n "$nextos_mirrorlist" ]; then
        extra_cmdline+=" extra_mirrorlist='$nextos_mirrorlist'"
    fi

    # cloudcone 特殊处理
    if is_grub_dir_linked; then
        finalos_cmdline+=" extra_link_grub_dir=1"
    fi
}

echo_tmp_ttys() {
    echo "console=ttyS0,115200n8 console=tty0"
}

get_entry_name() {
    printf 'reinstall ('
    printf '%s' "$distro"
    [ -n "$releasever" ] && printf ' %s' "$releasever"
    [ "$distro" = alpine ] && [ "$hold" = 1 ] && printf ' Live OS'
    printf ')'
}

build_nextos_cmdline() {
    # shellcheck disable=SC2154 # setos() assigns these target values dynamically.
    [ "$nextos_distro" = alpine ] || error_and_exit "Automatic targets must boot Alpine Live."
    # shellcheck disable=SC2154 # setos_alpine assigns these values dynamically.
    nextos_cmdline="alpine_repo=$nextos_repo modloop=$nextos_modloop $(echo_tmp_ttys)"
}

build_cmdline() {
    # nextos
    build_nextos_cmdline

    # finalos
    # trans 需要 finalos_distro 识别是安装 alpine 还是其他系统
    if [ "$distro" = alpine ]; then
        finalos_distro=alpine
    fi
    if [ -n "$finalos_distro" ]; then
        build_finalos_cmdline
    fi

    # extra
    build_extra_cmdline

    cmdline="$nextos_cmdline $finalos_cmdline $extra_cmdline"
}

set_boot_targets() {
    local alpine_ver=$1
    case "$distro" in
    alpine) setos nextos alpine "$releasever" ;;
    debian | arch | gentoo)
        [ -n "$alpine_ver" ] || error_and_exit "Alpine Live release was not resolved."
        setos finalos "$distro" "$releasever"
        setos nextos alpine "$alpine_ver"
        ;;
    netboot.xyz) setos nextos netboot.xyz ;;
    iso | reset) return 0 ;;
    *) error_and_exit "Unsupported boot target: $distro" ;;
    esac
}

# 脚本可能多次运行，先清理之前的残留
mkdir_clear() {
    local dir=$1

    if [ -z "$dir" ] || [ "$dir" = / ]; then
        return
    fi

    # 再次运行时，有可能 mount 了 btrfs root，因此先要 umount_all
    # 但目前不需要 mount ，因此用不到
    # umount_all "$dir"
    rm -rf "$dir"
    mkdir -p "$dir"
}

get_ip_conf_cmd() {
    collect_netconf >&2

    sh=/initrd-network.sh
    if is_found_ipv4_netconf && is_found_ipv6_netconf && [ "$ipv4_mac" = "$ipv6_mac" ]; then
        echo "'$sh' '$ipv4_mac' '$ipv4_addr' '$ipv4_gateway' '$ipv6_addr' '$ipv6_gateway' '$ipv6_extra_addrs'"
    else
        if is_found_ipv4_netconf; then
            echo "'$sh' '$ipv4_mac' '$ipv4_addr' '$ipv4_gateway' '' '' ''"
        fi
        if is_found_ipv6_netconf; then
            echo "'$sh' '$ipv6_mac' '' '' '$ipv6_addr' '$ipv6_gateway' '$ipv6_extra_addrs'"
        fi
    fi
}

is_need_web_viewer() {
    ! { [ "$distro" = netboot.xyz ] || is_alpine_live; }
}

mod_initrd_alpine() {
    # hack 1 v3.19 和之前的 virt 内核需添加 ipv6 模块
    if virt_dir=$(ls -d $initrd_dir/lib/modules/*-virt 2>/dev/null); then
        ipv6_dir=$virt_dir/kernel/net/ipv6
        if ! [ -f $ipv6_dir/ipv6.ko ] && ! grep -q ipv6 $initrd_dir/lib/modules/*/modules.builtin; then
            mkdir -p $ipv6_dir
            modloop_file=$tmp/modloop_file
            modloop_dir=$tmp/modloop_dir
            curl -Lo $modloop_file $nextos_modloop
            install_pkg unsquashfs
            mkdir_clear $modloop_dir
            unsquashfs -f -d $modloop_dir $modloop_file 'modules/*/kernel/net/ipv6/ipv6.ko'
            find $modloop_dir -name ipv6.ko -exec cp {} $ipv6_dir/ \;
        fi
    fi

    # hack 下载 dhcpcd
    # shellcheck disable=SC2154
    download_and_extract_apk "$nextos_releasever" dhcpcd "$initrd_dir"
    sed -i -e '/^slaac private/s/^/#/' -e '/^#slaac hwaddr/s/^#//' $initrd_dir/etc/dhcpcd.conf

    # hack 2 /usr/share/udhcpc/default.script
    # 脚本被调用的顺序
    # udhcpc:  deconfig
    # udhcpc:  bound
    # udhcpc6: deconfig
    # udhcpc6: bound
    # shellcheck disable=SC2329
    udhcpc() {
        if [ "$1" = deconfig ]; then
            return
        fi
        if [ "$1" = bound ] && [ -n "$ipv6" ]; then
            # shellcheck disable=SC2154
            ip -6 addr add "$ipv6" dev "$interface"
            ip link set dev "$interface" up
            return
        fi
    }

    get_function_content udhcpc |
        insert_into_file usr/share/udhcpc/default.script after 'deconfig\|renew\|bound'

    # 允许设置 ipv4 onlink 网关
    sed -Ei 's,(0\.0\.0\.0\/0),"\1 onlink",' usr/share/udhcpc/default.script

    # hack 3 网络配置
    # alpine 根据 MAC_ADDRESS 判断是否有网络
    # https://github.com/alpinelinux/mkinitfs/blob/c4c0115f9aa5aa8884c923dc795b2638711bdf5c/initramfs-init.in#L914
    insert_into_file init after 'configure_ip\(\)' <<EOF
        depmod
        [ -d /sys/module/ipv6 ] || modprobe ipv6
        $(get_ip_conf_cmd)
        MAC_ADDRESS=1
        return
EOF

    # grep -E -A5 'configure_ip\(\)' init

    # hack 4 run trans.start from Alpine Live after switch_root.
    # Connecting to raw.githubusercontent.com (185.199.108.133:443)
    # 60C0BB2FFAFF0000:error:0A00009C:SSL routines:ssl3_get_record:http request:ssl/record/ssl3_record.c:345:
    # ssl_client: SSL_connect
    # wget: bad header line: �
    insert_into_file init before '^exec switch_root' <<EOF
        # trans
        # echo "wget --no-check-certificate -O- $confhome/trans.sh | /bin/ash" >\$sysroot/etc/local.d/trans.start
        # wget --no-check-certificate -O \$sysroot/etc/local.d/trans.start $confhome/trans.sh
        cp /trans.sh \$sysroot/etc/local.d/trans.start
        chmod a+x \$sysroot/etc/local.d/trans.start
        ln -s /etc/init.d/local \$sysroot/etc/runlevels/default/

        # Linux installation configuration
        if [ -d /configs ]; then
            cp -r /configs \$sysroot/
            rm -rf /configs
        fi
EOF

    # 临时修复 liveos getty 运行在 tty0
    # shellcheck disable=SC2016
    if [ "$nextos_releasever" = 3.24 ] &&
        txt_to_grep='done < "$ROOT"/sys/class/tty/"$1"/active' &&
        grep -qF "$txt_to_grep" init; then
        insert_into_file init replace "$txt_to_grep" -F <<EOF
done < <(cat "\$ROOT"/sys/class/tty/"\$1"/active | xargs -n 1)
EOF
    fi
}

mod_initrd() {
    info "mod $nextos_distro initrd"
    install_pkg gzip cpio

    # 解压
    # 先删除临时文件，避免之前运行中断有残留文件
    initrd_dir=$tmp/initrd
    mkdir_clear $initrd_dir
    cd $initrd_dir

    # 解压/重新打包/删除 initrd 的 /dev/console /dev/null 都会报错
    # cpio: dev/console: Cannot utime: Invalid argument
    # cpio: ./dev/console: Cannot stat: Bad address

    # 但同样运行 zcat /reinstall-initrd | cpio -idm

    # shellcheck disable=SC2046
    # nonmatching 是精确匹配路径
    zcat /reinstall-initrd | cpio -idm

    curl -Lo $initrd_dir/trans.sh $confhome/trans.sh
    if ! grep -iq "$SCRIPT_VERSION" $initrd_dir/trans.sh; then
        error_and_exit "
This script is outdated, please download reinstall.sh again.
脚本有更新，请重新下载 reinstall.sh"
    fi

    curl -Lo $initrd_dir/initrd-network.sh $confhome/initrd-network.sh
    chmod a+x $initrd_dir/trans.sh $initrd_dir/initrd-network.sh

    # 保存配置
    mkdir -p $initrd_dir/configs
    if [ -n "$ssh_keys" ]; then
        cat <<<"$ssh_keys" >$initrd_dir/configs/ssh_keys
    else
        save_password $initrd_dir/configs
    fi
    if [ -n "$frpc_config" ]; then
        cat "$frpc_config" >$initrd_dir/configs/frpc.conf
    fi

    [ "$nextos_distro" = alpine ] || error_and_exit "Only Alpine Live initrds are supported."
    mod_initrd_alpine

    # alpine live 不精简 initrd
    # 因为不知道用户想干什么，可能会用到精简的文件
    if is_virt && ! is_alpine_live; then
        remove_useless_initrd_files
    fi

    if [ "$hold" = 0 ]; then
        info 'hold 0'
        echo "Edit $tmp if needed."
        read -r -p 'Press Enter to continue...'
    fi

    # 重建
    # 注意要用 cpio -H newc 不要用 cpio -c ，不同版本的 -c 作用不一样，很坑
    # -c    Use the old portable (ASCII) archive format
    # -c    Identical to "-H newc", use the new (SVR4)
    #       portable format.If you wish the old portable
    #       (ASCII) archive format, use "-H odc" instead.
    find . | cpio --quiet -o -H newc -R 0:0 | gzip -1 >/reinstall-initrd
    cd - >/dev/null
}

remove_useless_initrd_files() {
    info "slim initrd"

    # 显示精简前的大小
    du -sh .

    # 删除 initrd 里面没用的文件/驱动
    rm -rf bin/brltty
    rm -rf etc/brltty
    rm -rf sbin/wpa_supplicant
    rm -rf usr/lib/libasound.so.*
    rm -rf usr/share/alsa
    (
        cd lib/modules/*/kernel/drivers/net/ethernet/
        for item in *; do
            case "$item" in
            # Keep common VPS NIC modules, including the Mellanox VF driver.
            amazon | google | mellanox | realtek | pensando) ;;
            intel)
                (
                    cd "$item"
                    for sub_item in *; do
                        case "$sub_item" in
                        # 有 e100.ko e1000文件夹 e1000e文件夹
                        e100* | lib* | *vf | idpf) ;;
                        *) rm -rf $sub_item ;;
                        esac
                    done
                )
                ;;
            *) rm -rf $item ;;
            esac
        done
    )
    (
        cd lib/modules/*/kernel
        for item in \
            net/mac80211 \
            net/wireless \
            net/bluetooth \
            drivers/mtd \
            drivers/ssb \
            drivers/mfd \
            drivers/bcma \
            drivers/pcmcia \
            drivers/parport \
            drivers/platform \
            drivers/staging \
            drivers/net/usb \
            drivers/net/bonding \
            drivers/net/wireless \
            drivers/input/rmi4 \
            drivers/input/touchscreen \
            drivers/bus/mhi \
            drivers/char/pcmcia \
            drivers/misc/cardreader; do
            rm -rf $item
        done
    )

    # 显示精简后的大小
    du -sh .
}

get_unix_path() {
    printf '%s\n' "$1"
}

assert_safe_iso_path() {
    [[ "$1" =~ ^/[A-Za-z0-9._+@~/-]+$ ]] ||
        error_and_exit "ISO and internal paths must be absolute, without spaces or shell/GRUB metacharacters."
    case "/$1/" in */../* | */./*) error_and_exit "ISO paths must not contain dot segments." ;; esac
}

protect_iso_source() {
    [ -n "$iso" ] && [ -f "$iso" ] && [ -r "$iso" ] && [ -s "$iso" ] ||
        error_and_exit "--iso must name a readable, nonempty local ISO file."
    iso=$(readlink -f "$iso") || error_and_exit "Cannot resolve local ISO path."
    assert_safe_iso_path "$iso"
    case "$iso" in
    "$tmp" | "$tmp"/* | /reinstall-vmlinuz | /reinstall-initrd)
        error_and_exit "ISO source must not be inside managed temporary/boot files."
        ;;
    esac
}

validate_iso_args() {
    protect_iso_source
    if [ -n "$iso_kernel$iso_initrd" ]; then
        [ -n "$iso_kernel" ] && [ -n "$iso_initrd" ] || error_and_exit "Specify both --iso-kernel and --iso-initrd."
        [ -z "$iso_config" ] || error_and_exit "--iso-config conflicts with explicit kernel/initrd."
        assert_safe_iso_path "$iso_kernel"
        assert_safe_iso_path "$iso_initrd"
        # This is appended as GRUB tokens, never eval'ed. Reject command syntax.
        [[ "$iso_cmdline" =~ ^[A-Za-z0-9._~:/=,+@%!?*\ -]*$ ]] ||
            error_and_exit "ISO cmdline contains unsupported quoting or GRUB metacharacters."
    else
        [ -z "$iso_cmdline" ] || error_and_exit "--iso-cmdline requires --iso-kernel and --iso-initrd."
        iso_config=${iso_config:-/boot/grub/loopback.cfg}
        assert_safe_iso_path "$iso_config"
    fi
}

prepare_local_iso() {
    local prefix abstraction checkdir="$tmp/iso-check" invalid=
    iso_grub_prefix=
    for prefix in grub grub2; do
        if is_have_cmd "$prefix-probe" && is_have_cmd "$prefix-mkrelpath"; then
            iso_grub_prefix=$prefix
            break
        fi
    done
    [ -n "$iso_grub_prefix" ] || error_and_exit "Local ISO boot requires grub-probe and grub-mkrelpath (or grub2 equivalents)."
    abstraction=$("$iso_grub_prefix-probe" --target=abstraction "$iso") || error_and_exit "GRUB cannot inspect the ISO storage layout."
    if printf '%s\n' "$abstraction" | grep -q cryptodisk; then
        error_and_exit "Local ISO boot from encrypted storage is not supported."
    fi
    iso_uuid=$("$iso_grub_prefix-probe" --target=fs_uuid "$iso") || error_and_exit "GRUB cannot read the ISO filesystem UUID."
    [[ "$iso_uuid" =~ ^[A-Fa-f0-9-]+$ ]] || error_and_exit "Invalid ISO filesystem UUID."
    iso_path=$("$iso_grub_prefix-mkrelpath" "$iso") || error_and_exit "Cannot obtain the ISO's GRUB path."
    assert_safe_iso_path "$iso_path"
    # Read-only inspection. No repartitioning and no second copy of the large ISO.
    mkdir -p "$checkdir"
    mount -o loop,ro "$iso" "$checkdir" || error_and_exit "Cannot mount the local ISO read-only."
    if [ -f "$checkdir/sources/boot.wim" ] || [ -f "$checkdir/sources/install.wim" ] ||
        [ -f "$checkdir/sources/install.esd" ] || [ -f "$checkdir/SOURCES/BOOT.WIM" ] ||
        [ -f "$checkdir/SOURCES/INSTALL.WIM" ] || [ -f "$checkdir/SOURCES/INSTALL.ESD" ]; then
        invalid="This interface is for Linux installation media only."
    elif [ -n "$iso_kernel" ]; then
        [ -f "$checkdir$iso_kernel" ] && [ -f "$checkdir$iso_initrd" ] || invalid="Kernel/initrd path not found in the ISO."
    else
        [ -f "$checkdir$iso_config" ] || invalid="No loopback config found. Specify --iso-kernel, --iso-initrd and the ISO-specific media-discovery cmdline."
    fi
    umount "$checkdir" || error_and_exit "Cannot unmount ISO inspection directory."
    rmdir "$checkdir"
    [ -z "$invalid" ] || error_and_exit "$invalid"
    iso_cmdline=${iso_cmdline//%ISO_PATH%/$iso_path}
    iso_cmdline=${iso_cmdline//%ISO_UUID%/$iso_uuid}
}

render_local_iso_grub() {
    cat <<EOF
$BOOT_ENTEY_START_MARK
set timeout_style=menu
set timeout=5
EOF
    # Preserve the upstream CloudCone grubenv workaround for one-shot BIOS boot.
    get_function_content load_grubenv_if_not_loaded
    cat <<EOF
menuentry "$(get_entry_name)" --unrestricted {
    insmod lvm
    insmod mdraid1x
    insmod all_video
    insmod loopback
    set btrfs_relative_path=n
    search --no-floppy --fs-uuid --set=iso_device '$iso_uuid'
    set iso_path='$iso_path'
    set img_dev='/dev/disk/by-uuid/$iso_uuid'
    set img_loop='$iso_path'
    export iso_path img_dev img_loop
    loopback loop (\$iso_device)\$iso_path
    set root=(loop)
EOF
    if [ -n "$iso_kernel" ]; then
        printf '    linux (loop)%s %s\n    initrd (loop)%s\n' "$iso_kernel" "$iso_cmdline" "$iso_initrd"
    else
        printf '    configfile (loop)%s\n' "$iso_config"
    fi
    printf '}\n%s\n' "$BOOT_ENTEY_END_MARK"
}

boot_local_iso_and_exit() {
    is_use_local_extlinux && error_and_exit "Local ISO loopback boot requires GRUB, not extlinux."
    prepare_local_iso
    render_local_iso_grub >"$tmp/local-iso.grub.cfg"
    [ -s "$tmp/local-iso.grub.cfg" ] || error_and_exit "Empty ISO boot configuration."
    remove_exist_reinstall
    mkdir -p "$(dirname "$target_cfg")"
    cp "$tmp/local-iso.grub.cfg" "$target_cfg.tmp.$$"
    mv -f "$target_cfg.tmp.$$" "$target_cfg"
    if is_efi; then
        install_grub_linux_efi
    else
        "$grub-reboot" "$(get_entry_name)"
    fi
    info "Local ISO boot entry ready"
    echo "ISO: $iso (not copied or overwritten)"
    echo 'Reboot to use the ISO interactively through your provider console/VNC.'
    echo 'Do not erase the ISO-containing partition while the installer still needs the ISO.'
    echo 'Kernel/initrd support for loopback media is ISO-specific; this is not universal unattended installation.'
    echo "Run '$reinstall_____ reset' before reboot to cancel."
    exit 0
}

# This body is GRUB language, extracted into grub.cfg without executing it in Bash.
# shellcheck disable=SC2121,SC2154
load_grubenv_if_not_loaded() {
    if ! [ -s $prefix/grubenv ]; then
        for dir in /boot/grub /boot/grub2 /grub /grub2; do
            set grubenv="($root)$dir/grubenv"
            if [ -s $grubenv ]; then
                load_env --file $grubenv
                if [ "${next_entry}" ]; then
                    set default="${next_entry}"
                    set next_entry=
                    save_env --file $grubenv next_entry
                else
                    set default="0"
                fi
                return
            fi
        done
    fi
}

init_basearch() {
    basearch=$(uname -m)
    [ "$basearch" = x86_64 ] || error_and_exit "Unsupported architecture: $basearch (x86_64 only)."
    basearch_alt=amd64
}

init_confhome() {
    confhome=${confhome%/}
    [[ "$confhome" =~ ^https://[A-Za-z0-9._~:/-]+$ ]] ||
        error_and_exit "Set --confhome or REINSTALL_CONFHOME to the HTTPS base of your trimmed repository."
    case "$confhome" in
    https://raw.githubusercontent.com/bin456789/reinstall/* | https://cnb.cool/bin456789/reinstall/*)
        error_and_exit "Refusing untrimmed upstream runtime assets. Host your own trimmed repository."
        ;;
    esac
}

remove_exist_reinstall_efi_dir() {
    info "remove exist reinstall efi dir"

    local dir='' dirs=''
    dirs=$(get_maybe_efi_dirs_in_linux)
    # 后期可能会将 reinstall-vmlinuz 和 reinstall-initrd 放到 efi 分区下
    # 因此也删除它们
    for dir in $dirs; do
        rm -f "$dir/reinstall-vmlinuz"
        rm -f "$dir/reinstall-initrd"
    done
    find $dirs -type f \
        \( -ipath '*/EFI/reinstall/grubx64.efi' \
        -o -ipath '*/EFI/reinstall/netboot.xyz.efi' \) |
        while IFS= read -r efi_file; do
            reinstall_dir=$(dirname "$efi_file")
            echo "removing $reinstall_dir"
            rm -rf "$reinstall_dir"
        done
}

# bios        /boot/grub*/custom.cfg                     /cygdrive/c/grub/grub.cfg
# efi         efi分区的/EFI/reinstall/grub.cfg           /cygdrive/c/grub.cfg
# efi文件夹    efi分区的/EFI/reinstall/                  /cygdrive/a/EFI/reinstall/

init_bootloader_facts() {
    # linux
    if is_efi; then
        efi_dir=$(get_maybe_efi_dirs_in_linux | head -1)
        _grub_cfg=$efi_dir/EFI/reinstall/grub.cfg
        target_cfg=$_grub_cfg
    else
        if is_mbr_using_grub; then
            if is_have_cmd update-grub; then
                # alpine debian ubuntu
                _grub_cfg=$(grep -o '[^ ]*grub.cfg' "$(get_cmd_path update-grub)" | head -1)
            else
                # 找出主配置文件（含有menuentry|blscfg）
                # 有没有可能在 efi 目录?
                _grub_cfg=$(find_grub_extlinux_cfg '/boot/grub*' grub.cfg 'menuentry|blscfg')
            fi
            target_cfg=$(dirname $_grub_cfg)/custom.cfg

            if is_have_cmd grub2-mkconfig; then
                grub=grub2
            elif is_have_cmd grub-mkconfig; then
                grub=grub
            else
                error_and_exit "grub not found"
            fi
        else
            # extlinux
            _extlinux_cfg=$(find_grub_extlinux_cfg /boot extlinux.conf LINUX)
            target_cfg=$_extlinux_cfg
        fi
    fi
}

# 重新生成 grub.cfg
# 因为有些机子例如hython debian的grub.cfg少了40_custom 41_custom 部分
recreate_grub_or_extlinux_cfg() {
    # 没用到原机的 grub 和 extlinux
    # 因此不需要重新生成 grub.cfg 或 extlinux.conf
    if is_efi; then
        return
    fi

    if is_mbr_using_grub; then
        info "recreate grub.cfg"

        # nixos 手动执行 grub-mkconfig -o /boot/grub/grub.cfg 会丢失系统启动条目
        # 正确的方法是修改 configuration.nix 的 boot.loader.grub.extraEntries
        # 但是修改 configuration.nix 不是很好，因此改成修改 grub.cfg
        if [ -x /nix/var/nix/profiles/system/bin/switch-to-configuration ]; then
            # 生成 grub.cfg
            /nix/var/nix/profiles/system/bin/switch-to-configuration boot
            # 手动启用 41_custom
            nixos_grub_home="$(dirname "$(readlink -f "$(get_cmd_path grub-mkconfig)")")/.."
            $nixos_grub_home/etc/grub.d/41_custom >>"$(dirname "$target_cfg")/grub.cfg"
        elif is_have_cmd update-grub; then
            update-grub
        else
            $grub-mkconfig -o $target_cfg
        fi
    elif is_have_cmd update-extlinux; then
        # alpine 才有 update-extlinux
        info "recreate extlinux.conf"
        update-extlinux
    else
        error_and_exit "unsupported bootloader."
    fi
}

# 删除之前的 reinstall 启动项
remove_exist_reinstall() {
    info "remove exist reinstall"

    rm -f /reinstall-vmlinuz /reinstall-initrd
    rm -f /boot/reinstall-vmlinuz /boot/reinstall-initrd

    # 使用外部 grub 时，删除外部 grub.cfg
    if ! is_use_local_grub_extlinux; then
        rm -f "$target_cfg"
    fi

    if is_efi; then
        # efi

        # 题外话
        # 1. 如果用本机的 grub，则 custom.cfg 可能在 efi 分区，也可能在 /boot 分区
        # 2. 有可能没有 /boot 文件夹
        #    如果 nixos 的 efi 挂载到 /efi，则不会生成 /boot 文件夹
        # 3. find 不存在的路径会报错
        remove_exist_reinstall_efi_dir

        install_pkg efibootmgr
        efibootmgr | grep -q 'BootNext:' && efibootmgr --quiet --delete-bootnext
        efibootmgr | grep_efi_entry | grep 'reinstall' | grep_efi_index |
            xargs -I {} efibootmgr --quiet --bootnum {} --delete-bootnum
    else
        # bios

        # 删除 reinstall 条目
        if [ -f "$target_cfg" ]; then
            sed -i "/^$BOOT_ENTEY_START_MARK/,/^$BOOT_ENTEY_END_MARK/d" "$target_cfg"
        fi

        # 清除 next entry
        if is_use_local_grub; then
            $grub-editenv - unset next_entry
        elif is_use_local_extlinux; then
            extlinux --clear-once "$(dirname "$target_cfg")"
        fi

        # 重新创建 grub.cfg / extlinux.conf
        recreate_grub_or_extlinux_cfg
    fi
}

reset_and_exit() {
    from_ctrl_c=${1:-false}

    # info
    if $from_ctrl_c; then
        info "Caught Ctrl+C, reseting..."
    fi

    # 清除
    remove_exist_reinstall
    rm -rf "$tmp"
    echo "reset done."

    # 退出
    if $from_ctrl_c; then
        exit 1
    else
        exit 0
    fi
}

# 脚本入口

# Help must not require root, fetch metadata or initialize a bootloader.
for arg in "$@"; do
    case "$arg" in -h | --help) usage_and_exit 0 ;; esac
done
[ "$(uname -s)" = Linux ] || error_and_exit "Only Linux hosts are supported."
[ "$EUID" -eq 0 ] || error_and_exit "Please run as root."

if mount | grep -q 'tmpfs on / type tmpfs'; then
    error_and_exit "Can't run this script in Live OS."
fi
if is_in_container; then
    error_and_exit "Not Supported OS in Container.\nPlease use https://github.com/LloydAsp/OsMutation"
fi

# 整理参数
long_opts=
for o in debug no-cloud-kernel help hold: sleep: iso: iso-config: iso-kernel: iso-initrd: iso-cmdline: confhome: \
    user: username: passwd: password: ssh-port: ssh-key: public-key: web-port: http-port: \
    frpc-conf: frpc-config: target-disk: force-boot-mode:; do
    [ -n "$long_opts" ] && long_opts+=,
    long_opts+=$o
done

# 使用 getopt 解析参数
if ! ORIGINAL_OPTS=$(getopt -n $0 -o "h,x" --long "$long_opts" -- "$@"); then
    exit 1
fi

# 第一遍扫描，验证要安装的系统和版本
eval set -- "$ORIGINAL_OPTS"
while true; do
    case "$1" in
    -x | --debug)
        set -x
        shift
        ;;
    --confhome)
        confhome=$2
        shift 2
        ;;
    --iso)
        iso=$2
        shift 2
        ;;
    --)
        shift
        init_basearch
        case "$(printf '%s' "$1" | to_lower)" in
        alpine | debian | arch | gentoo) init_confhome ;;
        esac
        verify_os_name "$@"
        break
        ;;
    --no-cloud-kernel)
        shift
        ;;
    *)
        # Other accepted options take a value; never reinterpret that value as an option.
        shift 2
        ;;
    esac
done

# Protect the already-downloaded ISO before clearing the managed work directory.
[ ! -L "$tmp" ] || error_and_exit "Refusing symlink work directory: $tmp"
if [ "$distro" = iso ]; then
    protect_iso_source
fi
# 处理 --frpc-config 时会下载文件，因此在处理参数前就创建临时目录
mkdir_clear "$tmp"

# 安装必备组件
install_pkg curl grep

# 第二遍扫描，处理参数
eval set -- "$ORIGINAL_OPTS"
# shellcheck disable=SC2034
while true; do
    case "$1" in
    -x | --debug)
        # 第一遍扫描已处理
        shift
        ;;
    -h | --help)
        usage_and_exit 0
        ;;
    --confhome)
        # Already handled before version resolution.
        shift 2
        ;;
    --no-cloud-kernel)
        no_cloud_kernel=1
        shift
        ;;
    --hold | --sleep)
        if ! { [ "$2" = 0 ] || [ "$2" = 1 ] || [ "$2" = 2 ]; }; then
            error_and_exit "Invalid $1 value: $2"
        fi
        hold=$2
        shift 2
        ;;
    --frpc-conf | --frpc-config)
        [ -n "$2" ] || error_and_exit "Need value for $1"

        case "$(to_lower <<<"$2")" in
        http://* | https://*)
            frpc_config_url=$2
            frpc_config=$tmp/frpc.conf
            # 用 file 识别文件类型？
            if ! curl -L "$frpc_config_url" -o "$frpc_config"; then
                error_and_exit "Can't get frpc config from $frpc_config_url"
            fi
            ;;
        *)
            if ! { frpc_config=$(get_unix_path "$2") && [ -f "$frpc_config" ]; }; then
                error_and_exit "File not exists: $2"
            fi
            ;;
        esac

        # 转为绝对路径
        frpc_config=$(readlink -f "$frpc_config")

        shift 2
        ;;
    --force-boot-mode)
        if ! { [ "$2" = bios ] || [ "$2" = efi ]; }; then
            error_and_exit "Invalid $1 value: $2"
        fi
        force_boot_mode=$2
        shift 2
        ;;
    --user | --username)
        [ -n "$2" ] || error_and_exit "Need value for $1"
        username="$(printf "%s" "$2" | trim)"
        assert_username_valid
        shift 2
        ;;
    --passwd | --password)
        [ -n "$2" ] || error_and_exit "Need value for $1"
        password=$2
        shift 2
        ;;
    --ssh-key | --public-key)
        ssh_key_error_and_exit() {
            error "$1"
            cat <<EOF
Available options:
  --ssh-key "ssh-rsa ..."
  --ssh-key "ssh-ed25519 ..."
  --ssh-key "ecdsa-sha2-nistp256/384/521 ..."
  --ssh-key github:your_username
  --ssh-key gitlab:your_username
  --ssh-key http://path/to/public_key
  --ssh-key https://path/to/public_key
  --ssh-key /path/to/public_key
  --ssh-key C:\path\to\public_key
EOF
            exit 1
        }

        # https://manpages.debian.org/testing/openssh-server/authorized_keys.5.en.html#AUTHORIZED_KEYS_FILE_FORMAT
        is_valid_ssh_key() {
            grep -qE '^(ecdsa-sha2-nistp(256|384|521)|ssh-(ed25519|rsa)) ' <<<"$1"
        }

        [ -n "$2" ] || ssh_key_error_and_exit "Need value for $1"

        case "$(to_lower <<<"$2")" in
        gh:* | github:* | gl:* | gitlab:* | http://* | https://*)
            if [[ "$(to_lower <<<"$2")" = http* ]]; then
                key_url=$2
            else
                IFS=: read -r site user <<<"$2"
                case "$(to_lower <<<"$site")" in
                gh | github) site=github ;;
                gl | gitlab) site=gitlab ;;
                *) ;;
                esac
                [ -n "$user" ] || ssh_key_error_and_exit "Need a username for $site"
                key_url="https://$site.com/$user.keys"
            fi
            if ! ssh_key=$(curl -L "$key_url"); then
                error_and_exit "Can't get ssh key from $key_url"
            fi
            ;;
        *)
            # 检测值是否为 ssh key
            if is_valid_ssh_key "$2"; then
                ssh_key=$2
            else
                # 视为路径
                if ! { ssh_key_file=$(get_unix_path "$2") && [ -f "$ssh_key_file" ]; }; then
                    ssh_key_error_and_exit "SSH Key/File/Url \"$2\" is invalid."
                fi
                ssh_key=$(<"$ssh_key_file")
            fi
            ;;
        esac

        # 检查 key 格式
        if ! is_valid_ssh_key "$ssh_key"; then
            ssh_key_error_and_exit "SSH Key/File/Url \"$2\" is invalid."
        fi

        # 保存 key
        # 不用处理注释，可以支持写入 authorized_keys
        # 安装 nixos 时再处理注释/空行，转成数组，再添加到 nix 配置文件中
        if [ -n "$ssh_keys" ]; then
            ssh_keys+=$'\n'
        fi
        ssh_keys+=$ssh_key

        shift 2
        ;;
    --ssh-port)
        is_port_valid $2 || error_and_exit "Invalid $1 value: $2"
        ssh_port=$2
        shift 2
        ;;
    --web-port | --http-port)
        is_port_valid $2 || error_and_exit "Invalid $1 value: $2"
        web_port=$2
        shift 2
        ;;
    --target-disk)
        xda=${2##*/dev/}
        if ! [ -b "/dev/$xda" ]; then
            error_and_exit "Can't not find Disk $2."
        fi
        shift 2
        ;;
    --iso | --iso-config | --iso-kernel | --iso-initrd | --iso-cmdline)
        [ -n "$2" ] || error_and_exit "Need value for $1"
        case "$1" in
        --iso) iso=$2 ;;
        --iso-config) iso_config=$2 ;;
        --iso-kernel) iso_kernel=$2 ;;
        --iso-initrd) iso_initrd=$2 ;;
        --iso-cmdline) iso_cmdline=$2 ;;
        esac
        shift 2
        ;;
    --)
        shift
        break
        ;;
    *)
        echo "Unexpected option: $1."
        usage_and_exit
        ;;
    esac
done

# Validate before changing existing boot entries. reset must not require hosted assets.
verify_os_args
init_bootloader_facts
if [ "$distro" = reset ]; then
    reset_and_exit
fi
if is_secure_boot_enabled; then
    error_and_exit "Please disable secure boot first."
fi
if [ "$distro" = iso ]; then
    boot_local_iso_and_exit
fi

# 用户名
if ! is_netboot_xyz && [ -z "$username" ]; then
    prompt_username
fi

# 密码
if ! is_netboot_xyz && [ -z "$ssh_keys" ] && [ -z "$password" ]; then
    prompt_password
fi

# 检查内存
check_ram

# Every automatic target except the interactive netboot.xyz route uses Alpine Live.
alpine_ver_for_trans=
case "$distro" in
debian | arch | gentoo) alpine_ver_for_trans=$(get_latest_distro_releasever alpine) ;;
esac
set_boot_targets "$alpine_ver_for_trans"

# 有的机器开启了 kexec，例如腾讯云轻量 debian，要禁用
if [ -f /etc/default/kexec ]; then
    sed -i 's/LOAD_KEXEC=true/LOAD_KEXEC=false/' /etc/default/kexec
fi

# 一切就绪，正式下载内核和添加引导项
# 先删除之前的启动项，再设置 trap
# 暂时不用 trap，因为 bat 下无效
remove_exist_reinstall
# trap 'reset_and_exit true' SIGINT

# 下载 netboot.xyz / 内核
# shellcheck disable=SC2154
if is_netboot_xyz; then
    if is_efi; then
        add_efi_entry_in_linux $nextos_efi
    else
        curl -Lo /reinstall-vmlinuz $nextos_vmlinuz
    fi
else
    # 下载 nextos 内核
    info download vmlnuz and initrd
    curl -Lo /reinstall-vmlinuz $nextos_vmlinuz
    curl -Lo /reinstall-initrd $nextos_initrd
    mod_initrd
fi

# web 路径
if is_need_web_viewer; then
    web_path="/$(tr -dc "A-Za-z0-9" </dev/urandom | head -c8)"
fi

# 将内核/netboot.xyz.lkrn 放到正确的位置

# 需要使用 vmlinuz/initrd 引导的情况
if is_need_boot_vmlinuz; then
    # Use the system's external GRUB for Linux EFI boot.
    if is_efi; then
        install_grub_linux_efi
    fi

    # 找到 /reinstall-vmlinuz /reinstall-initrd 的绝对路径
    # extlinux + 单独的 boot 分区
    # 把内核文件放在 extlinux.conf 所在的目录
    if is_use_local_extlinux && is_boot_in_separate_partition; then
        dir=
    else
        # 获取当前系统根目录在 btrfs 中的绝对路径
        if is_os_in_btrfs; then
            # btrfs subvolume show /
            # 输出可能是 / 或 root 或 @/.snapshots/1/snapshot
            dir=$(btrfs subvolume show / | head -1)
            if ! [ "$dir" = / ]; then
                dir="/$dir/"
            fi
        else
            dir=/
        fi
    fi

    vmlinuz=${dir}reinstall-vmlinuz
    initrd=${dir}reinstall-initrd

    # 设置 linux initrd 命令
    if is_use_local_extlinux; then
        linux_cmd=LINUX
        initrd_cmd=INITRD
    else
        if is_netboot_xyz; then
            linux_cmd=linux16
            initrd_cmd=initrd16
        else
            linux_cmd=linux
            initrd_cmd=initrd
        fi
    fi

    # 设置 cmdlind initrds
    if ! is_netboot_xyz; then
        find_main_disk
        build_cmdline

        initrds="$initrd"
    fi

    if is_use_local_extlinux; then
        info extlinux
        echo "$target_cfg"
        extlinux_dir="$(dirname "$target_cfg")"

        # 不起作用
        # 好像跟 extlinux --once 有冲突
        sed -i "/^MENU HIDDEN/d" "$target_cfg"
        sed -i "/^TIMEOUT /d" "$target_cfg"

        del_empty_lines <<EOF | tee -a "$target_cfg"
$BOOT_ENTEY_START_MARK
TIMEOUT 5
LABEL reinstall
  MENU LABEL $(get_entry_name)
  $linux_cmd $vmlinuz
  $([ -n "$initrds" ] && echo "$initrd_cmd $initrds")
  $([ -n "$cmdline" ] && echo "APPEND $cmdline")
$BOOT_ENTEY_END_MARK
EOF
        # 设置重启引导项
        extlinux --once=reinstall $extlinux_dir

        # 复制文件到 extlinux 工作目录
        if is_boot_in_separate_partition; then
            info "copying files to $extlinux_dir"
            is_have_initrd && cp -f /reinstall-initrd $extlinux_dir
            # 放最后，防止前一条返回非 0 而报错
            cp -f /reinstall-vmlinuz $extlinux_dir
        fi
    else
        # cloudcone 从光驱的 grub 启动，再加载硬盘的 grub.cfg
        # menuentry "Grub 2" --id grub2 {
        #         set root=(hd0,msdos1)
        #         configfile /boot/grub2/grub.cfg
        # }

        # The shared GRUB-language helper reloads grubenv for CloudCone.

        # 生成 grub 配置
        # 实测 centos 7 lvm 要手动加载 lvm 模块
        info grub
        echo $target_cfg

        echo '### BEGIN reinstall.sh ###' >$target_cfg

        get_function_content load_grubenv_if_not_loaded >>$target_cfg

        # 原系统为 openeuler 云镜像，需要添加 --unrestricted，否则要输入密码
        del_empty_lines <<EOF | del_comment_lines | tee -a $target_cfg
set timeout_style=menu
set timeout=5
menuentry "$(get_entry_name)" --unrestricted {
    insmod lvm
    $(is_os_in_btrfs && echo 'set btrfs_relative_path=n')
    # fedora efi 没有 load_video
    insmod all_video
    # set gfxmode=800x600
    # set gfxpayload=keep
    # terminal_output gfxterm 在 vultr 上会花屏
    # terminal_output console
    search --no-floppy --file --set=root $vmlinuz
    $linux_cmd $vmlinuz $cmdline
    $([ -n "$initrds" ] && echo "$initrd_cmd $initrds")
}
EOF
        echo '### END reinstall.sh ###' >>$target_cfg

        # 设置重启引导项
        if is_use_local_grub; then
            $grub-reboot "$(get_entry_name)"
        fi
    fi
fi

info 'info'
echo "$distro $releasever"

ssh_port=${ssh_port:-22}
web_port=${web_port:-80}

if [ "$distro" = netboot.xyz ]; then
    :
elif [ "$distro" = alpine ] && [ "$hold" = 1 ]; then
    info "Alpine Live OS"
    echo "Username: $username"
    if [ -n "$ssh_keys" ]; then
        echo "Public Key: $ssh_keys"
    else
        echo "Password: $password"
    fi
    echo "SSH Port: $ssh_port"

else
    # 普通 linux
    info "While Install (View Logs)"
    echo "Username: $username"
    if [ -n "$ssh_keys" ]; then
        echo "Public Key: $ssh_keys"
    else
        echo "Password: $password"
    fi
    echo "SSH Port: $ssh_port"
    echo "WEB: $(get_http_log_url)"

    info "After Install"
    echo "Username: $username"
    if [ -n "$ssh_keys" ]; then
        echo "Public Key: $ssh_keys"
    else
        echo "Password: $password"
    fi
    echo "SSH Port: $ssh_port"
fi

echo
if [ "$distro" = netboot.xyz ]; then
    echo '重启后进入 netboot.xyz。'
    echo "或者现在运行 \"$reinstall_____ reset\" 以清除该引导项。"
    echo
    echo 'Reboot to start netboot.xyz.'
    echo "Or run \"$reinstall_____ reset\" now to clear this boot entry."
    echo

elif [ "$distro" = alpine ] && [ "$hold" = 1 ]; then
    echo '重启后进入 Alpine Live OS。'
    echo "或者现在运行 \"$reinstall_____ reset\" 以清除该引导项。"
    echo
    echo 'Reboot to start Alpine Live OS.'
    echo "Or run \"$reinstall_____ reset\" now to clear this boot entry."
    echo
else
    warn false '警告：重装会清除主硬盘的所有数据，包括所有分区！'
    echo '重启后开始重装。'
    echo "或者现在运行 \"$reinstall_____ reset\" 以取消重装。"
    echo
    warn false 'Warning: Reinstalling will erase all data on the main disk, including all partitions!'
    echo 'Reboot to start the reinstallation.'
    echo "Or run \"$reinstall_____ reset\" now to cancel the reinstallation."
fi
echo
