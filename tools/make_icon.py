"""AgentMux 应用图标：蓝色圆角底 + 两个对话气泡 + 向右分发的三条射线。

用脚本画而不是用位图编辑器：源文件可读、可改、可在 CI 里重生成。
跑法：py tools/make_icon.py  （在仓库根目录）
输出：src-tauri/icons/app-icon.png（1024 主图），随后交给 `cargo tauri icon` 生成全套。
"""

from PIL import Image, ImageDraw

S = 4  # 先按 4 倍画再缩，得到抗锯齿边缘（Pillow 的形状本身不做抗锯齿）
BASE = 1024
SIZE = BASE * S

TOP = (10, 132, 255)  # #0A84FF 亮蓝
BOTTOM = (0, 58, 99)  # #003A63 深蓝


def rounded_mask(size: int, radius: int) -> Image.Image:
    mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(mask).rounded_rectangle([0, 0, size - 1, size - 1], radius=radius, fill=255)
    return mask


def gradient(size: int) -> Image.Image:
    img = Image.new("RGB", (1, size))
    for y in range(size):
        t = y / (size - 1)
        img.putpixel(
            (0, y),
            tuple(round(TOP[i] + (BOTTOM[i] - TOP[i]) * t) for i in range(3)),
        )
    return img.resize((size, size), Image.BILINEAR)


def main() -> None:
    canvas = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    tile = gradient(SIZE).convert("RGBA")
    canvas.paste(tile, (0, 0), rounded_mask(SIZE, round(224 * S)))

    draw = ImageDraw.Draw(canvas, "RGBA")

    def px(value: int) -> int:
        return round(value * S)

    # 后面的气泡：半透明白，做出层次（不描边，小尺寸下更干净）
    draw.rounded_rectangle(
        [px(228), px(292), px(566), px(556)],
        radius=px(74),
        fill=(255, 255, 255, 208),
    )
    draw.polygon(
        [(px(300), px(520)), (px(300), px(636)), (px(392), px(552))],
        fill=(255, 255, 255, 208),
    )

    # 前面的气泡：纯白，向右上/右/右下各引一条粗线（表示把一条消息分发出去）
    draw.line(
        [(px(766), px(512)), (px(902), px(360))],
        fill=(255, 255, 255, 255),
        width=px(38),
    )
    draw.line(
        [(px(766), px(512)), (px(940), px(512))],
        fill=(255, 255, 255, 255),
        width=px(38),
    )
    draw.line(
        [(px(766), px(512)), (px(902), px(664))],
        fill=(255, 255, 255, 255),
        width=px(38),
    )
    for point in [(902, 360), (940, 512), (902, 664)]:
        r = px(19)
        draw.ellipse(
            [px(point[0]) - r, px(point[1]) - r, px(point[0]) + r, px(point[1]) + r],
            fill=(255, 255, 255, 255),
        )

    draw.rounded_rectangle(
        [px(384), px(372), px(766), px(652)],
        radius=px(74),
        fill=(255, 255, 255, 255),
    )
    draw.polygon(
        [(px(456), px(612)), (px(456), px(726)), (px(548), px(648))],
        fill=(255, 255, 255, 255),
    )

    icon = canvas.resize((BASE, BASE), Image.LANCZOS)
    icon.save("src-tauri/icons/app-icon.png")
    print("wrote src-tauri/icons/app-icon.png", icon.size)


if __name__ == "__main__":
    main()
