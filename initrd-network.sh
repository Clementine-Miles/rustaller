#!/bin/ash
# shellcheck shell=dash
# Alpine Live initrd networking helper

# accept_ra 接收 RA + 自动配置网关
# autoconf  自动配置地址，依赖 accept_ra

mac_addr=$1
ipv4_addr=$2
ipv4_gateway=$3
ipv6_addr=$4
ipv6_gateway=$5
ipv6_extra_addrs=$6

DHCP_TIMEOUT=15
DNS_FILE_TIMEOUT=5
TEST_TIMEOUT=10

# 检测是否有网络是通过检测这些 DNS endpoint 的端口是否开放
# 改成 generate_204？但检测网络时可能 resolv.conf 为空
# HTTP 80
# HTTPS/DOH 443
# DOT 853
ipv4_dns1='1.1.1.1'
ipv4_dns2='8.8.8.8' # 不开放 80
ipv6_dns1='2606:4700:4700::1111'
ipv6_dns2='2001:4860:4860::8888' # 不开放 80

# 找到主网卡
get_ethx() {
    # 过滤 azure vf (带 master ethx)
    # 2: eth0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 qdisc mq state UP qlen 1000\    link/ether 60:45:bd:21:8a:51 brd ff:ff:ff:ff:ff:ff
    # 3: eth1: <BROADCAST,MULTICAST,UP,LOWER_UP800> mtu 1500 qdisc mq master eth0 state UP qlen 1000\    link/ether 60:45:bd:21:8a:51 brd ff:ff:ff
    ip -o link | grep -i "$mac_addr" | grep -v master | cut -d' ' -f2 | cut -d: -f1 | grep .
}

get_ipv4_gateway() {
    ip -4 route show default dev "$ethx" | head -1 | cut -d ' ' -f3
}

get_ipv6_gateway() {
    ip -6 route show default dev "$ethx" | head -1 | cut -d ' ' -f3
}

get_first_ipv4_addr() {
    ip -4 -o addr show scope global dev "$ethx" | head -1 | grep -o '[0-9\.]*/[0-9]*'
}

get_first_ipv4_gateway() {
    ip -4 route show default dev "$ethx" | head -1 | cut -d' ' -f3
}

remove_netmask() {
    cut -d/ -f1
}

get_first_ipv6_addr() {
    ip -6 -o addr show scope global dev "$ethx" | head -1 | grep -o '[0-9a-f\:]*/[0-9]*'
}

get_first_ipv6_gateway() {
    ip -6 route show default dev "$ethx" | head -1 | cut -d' ' -f3
}

is_have_ipv4_addr() {
    ip -4 addr show scope global dev "$ethx" | grep -q inet
}

is_have_ipv6_addr() {
    ip -6 addr show scope global dev "$ethx" | grep -q inet6
}

is_have_ipv4_gateway() {
    ip -4 route show default dev "$ethx" | grep -q .
}

is_have_ipv6_gateway() {
    ip -6 route show default dev "$ethx" | grep -q .
}

is_have_ipv4() {
    is_have_ipv4_addr && is_have_ipv4_gateway
}

is_have_ipv6() {
    is_have_ipv6_addr && is_have_ipv6_gateway
}

is_have_ipv4_dns() {
    [ -f /etc/resolv.conf ] && grep -q '^nameserver .*\.' /etc/resolv.conf
}

is_have_ipv6_dns() {
    [ -f /etc/resolv.conf ] && grep -q '^nameserver .*:' /etc/resolv.conf
}

add_missing_ipv4_config() {
    if [ -n "$ipv4_addr" ] && [ -n "$ipv4_gateway" ]; then
        if ! is_have_ipv4_addr; then
            ip -4 addr add "$ipv4_addr" dev "$ethx"
        fi

        if ! is_have_ipv4_gateway; then
            # 如果 dhcp 无法设置onlink网关，那么在这里设置
            if true; then
                ip -4 route add "$ipv4_gateway" dev "$ethx"
                ip -4 route add default via "$ipv4_gateway" dev "$ethx"
            else
                ip -4 route add default via "$ipv4_gateway" dev "$ethx" onlink
            fi
        fi
    fi
}

add_missing_ipv6_config() {
    if [ -n "$ipv6_addr" ] && [ -n "$ipv6_gateway" ]; then
        if ! is_have_ipv6_addr; then
            ip -6 addr add "$ipv6_addr" dev "$ethx"
        fi

        if ! is_have_ipv6_gateway; then
            # 如果 dhcp 无法设置onlink网关，那么在这里设置
            if true; then
                ip -6 route add "$ipv6_gateway" dev "$ethx"
                ip -6 route add default via "$ipv6_gateway" dev "$ethx"
            else
                ip -6 route add default via "$ipv6_gateway" dev "$ethx" onlink
            fi
        fi

        # 添加额外的 IPv6 地址（逗号分隔）
        # 在腾讯云上，dhcpv6 获取的 IPv6 不是旧系统的 IPv6，且不可用
        # 脚本添加旧系统的 IPv6 地址和额外的 IPv6 地址后，额外的 IPv6 地址在 ip -6 addr 第一位
        # get_first_ipv6_addr 获得的是额外的 IPv6 地址，也就是不可用的 IPv6 地址
        # 导致 ipv6 test_internet 失败
        # 因此暂时关闭 ipv6_extra_addrs
    fi
}

is_need_test_ipv4() {
    is_have_ipv4 && ! $ipv4_has_internet
}

is_need_test_ipv6() {
    is_have_ipv6 && ! $ipv6_has_internet
}

# nc 探测 TCP 443 是否可达

test_connect() {
    src=$1
    dst=$2
    nc -z -v -w "$TEST_TIMEOUT" -s "$src" "$dst" 443
}

test_internet() {
    for i in $(seq 5); do
        echo "Testing Internet Connection. Test $i... "
        if is_need_test_ipv4 &&
            current_ipv4_addr="$(get_first_ipv4_addr | remove_netmask)" &&
            { test_connect "$current_ipv4_addr" "$ipv4_dns1" ||
                test_connect "$current_ipv4_addr" "$ipv4_dns2"; } >/dev/null 2>&1; then
            echo "IPv4 has internet."
            ipv4_has_internet=true
        fi
        if is_need_test_ipv6 &&
            current_ipv6_addr="$(get_first_ipv6_addr | remove_netmask)" &&
            { test_connect "$current_ipv6_addr" "$ipv6_dns1" ||
                test_connect "$current_ipv6_addr" "$ipv6_dns2"; } >/dev/null 2>&1; then
            echo "IPv6 has internet."
            ipv6_has_internet=true
        fi
        if ! is_need_test_ipv4 && ! is_need_test_ipv6; then
            break
        fi
        sleep 1
    done
}

flush_ipv4_config() {
    ip -4 addr flush scope global dev "$ethx"
    ip -4 route flush dev "$ethx"
    # DHCP 获取的 IP 不是重装前的 IP 时，一并删除 DHCP 获取的 DNS，以防 DNS 无效
    sed -i "/\./d" /etc/resolv.conf
}

should_disable_dhcpv4=false
should_disable_accept_ra=false
should_disable_autoconf=false

flush_ipv6_config() {
    if $should_disable_accept_ra; then
        echo 0 >"/proc/sys/net/ipv6/conf/$ethx/accept_ra"
    fi
    if $should_disable_autoconf; then
        echo 0 >"/proc/sys/net/ipv6/conf/$ethx/autoconf"
    fi
    ip -6 addr flush scope global dev "$ethx"
    ip -6 route flush dev "$ethx"
    # DHCP 获取的 IP 不是重装前的 IP 时，一并删除 DHCP 获取的 DNS，以防 DNS 无效
    sed -i "/:/d" /etc/resolv.conf
}

for i in $(seq 20); do
    if ethx=$(get_ethx); then
        break
    fi
    sleep 1
done

if [ -z "$ethx" ]; then
    echo "Not found network card: $mac_addr"
    exit
fi

echo "Configuring $ethx ($mac_addr)..."

# 不开启 lo 则 frp 无法连接 127.0.0.1 22
ip link set dev lo up

# 开启 ethx
ip link set dev "$ethx" up
sleep 1

# Alpine Live DHCPv4/v6. A timeout bounds providers' noisy udhcpc retries.
# dhcpcd may expire its lease when stopped, so use udhcpc for this one-shot probe.
method=udhcpc
case "$method" in
udhcpc)
    timeout "$DHCP_TIMEOUT" udhcpc -i "$ethx" -f -q -n || true
    timeout "$DHCP_TIMEOUT" udhcpc6 -i "$ethx" -f -q -n || true
    sleep "$DNS_FILE_TIMEOUT"
    ;;
dhcpcd)
    grep -q dhcpcd /etc/group || addgroup -S dhcpcd
    grep -q dhcpcd /etc/passwd || adduser -S -D -H \
        -h /var/lib/dhcpcd \
        -s /sbin/nologin \
        -G dhcpcd \
        -g dhcpcd \
        dhcpcd
    dhcpcd --persistent --noipv4ll "$ethx"
    sleep "$DNS_FILE_TIMEOUT"
    dhcpcd -x "$ethx"
    sysctl -w "net.ipv6.conf.$ethx.autoconf=1"
    sysctl -w "net.ipv6.conf.$ethx.accept_ra=1"
    ;;
esac

# 等待slaac
# 有ipv6地址就跳过，不管是slaac或者dhcpv6
# 因为会在trans里判断
# 这里等待5秒就够了，因为之前尝试获取dhcp6也用了一段时间
for i in $(seq 5 -1 0); do
    is_have_ipv6 && break
    echo "waiting slaac for ${i}s"
    sleep 1
done

# 记录是否有动态地址
# 由于还没设置静态ip，所以有条目表示有动态地址
is_have_ipv4_addr && dhcpv4=true || dhcpv4=false
is_have_ipv6_addr && dhcpv6_or_slaac=true || dhcpv6_or_slaac=false
is_have_ipv6_gateway && ra_has_gateway=true || ra_has_gateway=false

# 如果自动获取的 IP 不是重装前的，则改成静态，使用之前的 IP
# 只比较 IP，不比较掩码/网关，因为
# 1. 假设掩码/网关导致无法上网，后面也会检测到并改成静态
# 2. openSUSE wicked dhcpv6 是 64 位掩码，aws lightsail 模板上的也是，而其它 dhcpv6 软件都是 128 位掩码
if $dhcpv4 && [ -n "$ipv4_addr" ] && [ -n "$ipv4_gateway" ] &&
    ! [ "$(echo "$ipv4_addr" | cut -d/ -f1)" = "$(get_first_ipv4_addr | cut -d/ -f1)" ]; then
    echo "IPv4 address obtained from DHCP is different from old system."
    should_disable_dhcpv4=true
    flush_ipv4_config
fi
if $dhcpv6_or_slaac && [ -n "$ipv6_addr" ] && [ -n "$ipv6_gateway" ] &&
    ! [ "$(echo "$ipv6_addr" | cut -d/ -f1)" = "$(get_first_ipv6_addr | cut -d/ -f1)" ]; then
    echo "IPv6 address obtained from SLAAC/DHCPv6 is different from old system."
    should_disable_accept_ra=true
    should_disable_autoconf=true
    flush_ipv6_config
fi

add_missing_ipv4_config
add_missing_ipv6_config

# 检查 ipv4/ipv6 是否连接联网
ipv4_has_internet=false
ipv6_has_internet=false
test_internet

# 如果无法上网，并且自动获取的 掩码/网关 不是重装前的，则改成静态
# ip_addr 包括 IP/掩码，所以可以用来判断掩码是否不同
# IP 不同的情况在前面已经改成静态了
if ! $ipv4_has_internet &&
    $dhcpv4 && [ -n "$ipv4_addr" ] && [ -n "$ipv4_gateway" ] &&
    ! { [ "$ipv4_addr" = "$(get_first_ipv4_addr)" ] && [ "$ipv4_gateway" = "$(get_first_ipv4_gateway)" ]; }; then
    echo "IPv4 netmask/gateway obtained from DHCP is different from old system."
    should_disable_dhcpv4=true
    flush_ipv4_config
    add_missing_ipv4_config
    test_internet
fi
# 有可能是静态 IPv6 但能从 RA 获取到网关，因此加上 || $ra_has_gateway
if ! $ipv6_has_internet &&
    { $dhcpv6_or_slaac || $ra_has_gateway; } &&
    [ -n "$ipv6_addr" ] && [ -n "$ipv6_gateway" ] &&
    ! { [ "$ipv6_addr" = "$(get_first_ipv6_addr)" ] && [ "$ipv6_gateway" = "$(get_first_ipv6_gateway)" ]; }; then
    echo "IPv6 netmask/gateway obtained from SLAAC/DHCPv6 is different from old system."
    should_disable_accept_ra=true
    should_disable_autoconf=true
    flush_ipv6_config
    add_missing_ipv6_config
    test_internet
fi

# 要删除不联网协议的ip，因为
# 1 甲骨文云管理面板添加ipv6地址然后取消
#   依然会分配ipv6地址，但ipv6没网络
#   此时alpine只会用ipv6下载apk，而不用会ipv4下载
# 2 有ipv4地址但没有ipv4网关的情况(vultr $2.5 ipv6 only)，aria2会用ipv4下载

# 假设 ipv4 ipv6 在不同网卡，ipv4 能上网但 ipv6 不能上网，这时也要删除 ipv6
# 不能用 ipv4_has_internet && ! ipv6_has_internet 判断，因为它判断的是同一个网卡
if ! $ipv4_has_internet; then
    if $dhcpv4; then
        should_disable_dhcpv4=true
    fi
    flush_ipv4_config
fi
if ! $ipv6_has_internet; then
    # 防止删除 IPv6 后再次通过 SLAAC 获得
    # 不用判断 || $ra_has_gateway ，因为没有 IPv6 地址但有 IPv6 网关时，不会出现下载问题
    if $dhcpv6_or_slaac; then
        should_disable_accept_ra=true
        should_disable_autoconf=true
    fi
    flush_ipv6_config
fi

# 如果联网了，但没获取到默认 DNS，则添加我们的 DNS

# 有一种情况是，多网卡，且能上网的网卡先完成了这个脚本，不能上网的网卡后完成
# 无法上网的网卡通过 flush_ipv4_config 删除了不能上网的 IP 和 dns
# （原计划是删除无法上网的网卡 dhcp4 获取的 dns，但实际上无法区分）
# 因此这里直接添加 dns，不判断是否联网
if ! is_have_ipv4_dns; then
    echo "nameserver $ipv4_dns1" >>/etc/resolv.conf
    echo "nameserver $ipv4_dns2" >>/etc/resolv.conf
fi
if ! is_have_ipv6_dns; then
    echo "nameserver $ipv6_dns1" >>/etc/resolv.conf
    echo "nameserver $ipv6_dns2" >>/etc/resolv.conf
fi

# 传参给 trans.start
netconf="/dev/netconf/$ethx"
mkdir -p "$netconf"
$dhcpv4 && echo 1 >"$netconf/dhcpv4" || echo 0 >"$netconf/dhcpv4"
$dhcpv6_or_slaac && echo 1 >"$netconf/dhcpv6_or_slaac" || echo 0 >"$netconf/dhcpv6_or_slaac"
$should_disable_dhcpv4 && echo 1 >"$netconf/should_disable_dhcpv4" || echo 0 >"$netconf/should_disable_dhcpv4"
$should_disable_accept_ra && echo 1 >"$netconf/should_disable_accept_ra" || echo 0 >"$netconf/should_disable_accept_ra"
$should_disable_autoconf && echo 1 >"$netconf/should_disable_autoconf" || echo 0 >"$netconf/should_disable_autoconf"
echo "$ethx" >"$netconf/ethx"
echo "$mac_addr" >"$netconf/mac_addr"
echo "$ipv4_addr" >"$netconf/ipv4_addr"
echo "$ipv4_gateway" >"$netconf/ipv4_gateway"
echo "$ipv6_addr" >"$netconf/ipv6_addr"
echo "$ipv6_gateway" >"$netconf/ipv6_gateway"
echo "$ipv6_extra_addrs" >"$netconf/ipv6_extra_addrs"
$ipv4_has_internet && echo 1 >"$netconf/ipv4_has_internet" || echo 0 >"$netconf/ipv4_has_internet"
$ipv6_has_internet && echo 1 >"$netconf/ipv6_has_internet" || echo 0 >"$netconf/ipv6_has_internet"
