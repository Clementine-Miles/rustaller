# Strict family-aware parsing. No DNS lookup, libc shorthand, zones, or octal.
dmb_ipv4_words() {
  local ip=${1-} x; local -a words
  [[ $ip =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]] || return 64
  IFS=. read -r -a words <<< "$ip"
  for x in "${words[@]}"; do
    [[ $x == 0 || $x =~ ^[1-9][0-9]{0,2}$ ]] || return 64
    (( 10#$x <= 255 )) || return 64
  done
  printf '%d %d %d %d\n' "${words[@]}"
}
dmb_ipv6_words() {
  local ip=${1-} left right x v4 tail missing; local -a a=() b=() ipv6_expanded=() w=()
  [[ $ip == *:* && $ip != *[!0-9a-fA-F:.]* ]] || return 64
  if [[ $ip == *.* ]]; then
    tail=${ip##*:}; v4=$(dmb_ipv4_words "$tail") || return 64
    IFS=' ' read -r -a w <<< "$v4"
    printf -v tail '%x:%x' "$((w[0]*256+w[1]))" "$((w[2]*256+w[3]))"
    ip=${ip%:*}:$tail
  fi
  if [[ $ip == *::* ]]; then
    left=${ip%%::*}; right=${ip#*::}; [[ $right != *::* ]] || return 64
    [[ -z $left ]] || IFS=: read -r -a a <<< "$left"
    [[ -z $right ]] || IFS=: read -r -a b <<< "$right"
    missing=$((8-${#a[@]}-${#b[@]})); ((missing>=1)) || return 64
  else
    [[ $ip != :* && $ip != *: ]] || return 64
    IFS=: read -r -a a <<< "$ip"; ((${#a[@]}==8)) || return 64
    missing=0
  fi
  for x in "${a[@]}"; do [[ $x =~ ^[a-fA-F0-9]{1,4}$ ]] || return 64; ipv6_expanded+=("$((16#$x))"); done
  while ((missing>0)); do ipv6_expanded+=(0); ((missing--)); done
  for x in "${b[@]}"; do [[ $x =~ ^[a-fA-F0-9]{1,4}$ ]] || return 64; ipv6_expanded+=("$((16#$x))"); done
  ((${#ipv6_expanded[@]}==8)) || return 64
  printf '%s\n' "${ipv6_expanded[*]}"
}
dmb_ipv6_render() {
  local -a w=("$@"); local i start=-1 best=-1 len=0 longest=1 rendered='' hex
  for ((i=0;i<=8;i++)); do
    if ((i<8 && w[i]==0)); then ((start>=0)) || start=$i; ((len++))
    else
      if ((len>longest)); then best=$start; longest=$len; fi
      start=-1; len=0
    fi
  done
  for ((i=0;i<8;i++)); do
    if ((i==best)); then rendered+='::'; i=$((i+longest-1)); continue; fi
    printf -v hex '%x' "${w[i]}"
    [[ -z $rendered || $rendered == *: ]] || rendered+=:
    rendered+=$hex
  done
  printf '%s\n' "$rendered"
}
dmb_ip_parse() {
  local ip=${1-} family=${2-} words; local -a w
  if [[ $ip == *:* ]]; then
    [[ -z $family || $family == 6 ]] || return 64
    words=$(dmb_ipv6_words "$ip") || return; IFS=' ' read -r -a w <<< "$words"; dmb_ipv6_render "${w[@]}"
  else
    [[ -z $family || $family == 4 ]] || return 64
    words=$(dmb_ipv4_words "$ip") || return; IFS=' ' read -r -a w <<< "$words"; printf '%d.%d.%d.%d\n' "${w[@]}"
  fi
}
dmb_cidr_parse() {
  local cidr=${1-} family=${2-} ip bits words width max i keep; local -a w
  [[ $cidr == */* && $cidr != */*/* ]] || return 64
  ip=${cidr%/*}; bits=${cidr##*/}; [[ $bits == 0 || $bits =~ ^[1-9][0-9]{0,2}$ ]] || return 64
  ip=$(dmb_ip_parse "$ip" "$family") || return
  if [[ $ip == *:* ]]; then width=16; max=128; words=$(dmb_ipv6_words "$ip"); else width=8; max=32; words=$(dmb_ipv4_words "$ip"); fi
  ((bits<=max)) || return 64; IFS=' ' read -r -a w <<< "$words"
  for ((i=0;i<${#w[@]};i++)); do keep=$((bits-i*width)); if ((keep<=0)); then w[i]=0; elif ((keep<width)); then w[i]=$((w[i] & (((1<<keep)-1)<<(width-keep)))); fi; done
  if ((width==16)); then ip=$(dmb_ipv6_render "${w[@]}"); else printf -v ip '%d.%d.%d.%d' "${w[@]}"; fi
  printf '%s/%d\n' "$ip" "$bits"
}
dmb_ip_in_cidr() {
  local ip=$1 cidr=$2 canonical actual
  canonical=$(dmb_cidr_parse "$cidr") || return
  actual=$(dmb_cidr_parse "$ip/${canonical##*/}") || return
  [[ $canonical == "$actual" ]]
}
