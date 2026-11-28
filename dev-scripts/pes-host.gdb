set confirm off
set pagination off
handle SIGSEGV nostop noprint pass
break touchHLE_DynarmicWrapper_new
run
set $vm = $rdi
printf "Guest VM base = %p\n", $vm
watch -loc *(unsigned int*)($vm + 0x1007ce0)
commands
silent
printf "MUTEX WRITE SIG=%x hostpc=%p\n", *(unsigned int*)($vm+0x1007ce0), $pc
if *(unsigned int*)($vm+0x1007ce0) == 0
x/16wx $r15
set $guestsp = *(unsigned*)($r15+52)
x/24wx $vm+$guestsp
end
bt 7
x/7i $pc
info registers
continue
end
continue
