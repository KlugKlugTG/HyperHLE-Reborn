set confirm off
set pagination off
set breakpoint pending on
set language c
handle SIGSEGV nostop noprint pass
break 'HyperHLE::dyld::unimplemented_function_stub'
commands
silent
set $p = *(char**)&'HyperHLE::environment::LAST_HOST_CALL_SYMBOL_PTR'
set $n = *(unsigned long*)&'HyperHLE::environment::LAST_HOST_CALL_SYMBOL_LEN'
printf "UNIMPLEMENTED CALLED %.*s\n", (int)$n, $p
continue
end
info breakpoints
run
