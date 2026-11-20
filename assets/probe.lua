-- Print the source size once the first frame is configured. mpv exits after --frames=1.
mp.register_event("video-reconfig", function()
    local w = mp.get_property_number("video-params/dw")
    local h = mp.get_property_number("video-params/dh")
    if w and h and w > 0 and h > 0 then
        if (mp.get_property_number("video-params/rotate") or 0) % 180 == 90 then
            w, h = h, w
        end
        print(string.format("MULTIPAPR:%d:%d", math.floor(w + 0.5), math.floor(h + 0.5)))
    end
end)
