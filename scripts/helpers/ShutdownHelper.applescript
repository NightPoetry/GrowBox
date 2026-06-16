-- GrowBox 关机 helper(疫苗式 OS 授权体系的第一个 helper)。
-- 编译成签名的 GrowBox.app/Contents/Helpers/ShutdownHelper.app,有自己稳定的 TCC 身份。
-- 走 System Events 自动化(同意型 TCC,授权一次永久记住),免 root。
--   probe(探针/疫苗):无害读一次进程数 → 触发/确认"控制 System Events"授权弹窗,不改变任何东西。
--   shutdown [delaySecs]:可选延时后,经已授权的自动化通道关机(detached 启动则存活过 GrowBox 退出)。
on run argv
	set mode to "probe"
	if (count of argv) > 0 then set mode to item 1 of argv
	if mode is "shutdown" then
		set d to 0
		if (count of argv) > 1 then
			try
				set d to (item 2 of argv) as integer
			end try
		end if
		if d > 0 then delay d
		tell application "System Events" to shut down
		return "shutdown-issued"
	else
		-- 疫苗:发一条无害的同类 AppleEvent。未授权时此句报错 → applet 非零退出(可据此判"还没授权")。
		tell application "System Events" to count processes
		return "probe-ok"
	end if
end run
