set pagination off
set confirm off
set architecture arm
set endian little
target remote 127.0.0.1:23450
break *0x1ca044
commands
silent
printf "AT %x", $pc
info registers
x/16wx $sp
continue
end
break *0x247dc4
commands
silent
printf "AT %x", $pc
info registers
x/16wx $sp
continue
end
break *0x1d41b4
commands
silent
printf "AT %x", $pc
info registers
x/16wx $sp
continue
end
break *0x212b78
commands
silent
printf "AT %x", $pc
info registers
x/16wx $sp
continue
end
break *0x223258
commands
silent
printf "AT %x", $pc
info registers
x/16wx $sp
continue
end
break *0x710e8
commands
silent
printf "AT %x", $pc
info registers
x/16wx $sp
continue
end
break *0x212b7c
commands
silent
printf "AT %x", $pc
info registers
x/16wx $sp
continue
end
break *0x74400
commands
silent
printf "AT %x", $pc
info registers
x/16wx $sp
continue
end
break *0x212b88
commands
silent
printf "AT %x", $pc
info registers
x/16wx $sp
continue
end
break *0x6f944
commands
silent
printf "AT %x", $pc
info registers
x/16wx $sp
continue
end
continue
