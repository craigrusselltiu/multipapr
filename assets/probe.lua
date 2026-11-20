mp.register_event("video-reconfig", function()
    local w = mp.get_property_number("dwidth") or mp.get_property_number("video-out-params/dw")
    local h = mp.get_property_number("dheight") or mp.get_property_number("video-out-params/dh")
    if w and h and w > 0 and h > 0 then
        print(string.format("MULTIPAPR:%d:%d", math.floor(w + 0.5), math.floor(h + 0.5)))
        mp.command("quit 0")
    end
end)
