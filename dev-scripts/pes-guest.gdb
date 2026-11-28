set pagination off
set confirm off
set architecture arm
set endian little
target remote 127.0.0.1:23450
break *0x226a0
continue
info registers
x/12wx $sp
delete breakpoints
break *0x22760
continue
info registers
x/16wx $r3
x/16wx $sp
set $entry = *(unsigned*)($r3+0xc)
x/16wx $entry
stepi
info registers
x/12i $pc
set $i = 0
while $i < 230
  x/i $pc
  stepi
  set $i = $i + 1
end
continue
