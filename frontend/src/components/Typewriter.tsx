import { useEffect, useRef, useState, type CSSProperties } from "react";

/** 整段最长揭示时长；再长的文本也在这个时间内出完，不然粒子感等得心焦。 */
const MAX_TOTAL_MS = 1500;
/** 每字最长停留；短文本才有"打字"的观感，不至于一闪而过。 */
const MAX_PER_CHAR_MS = 30;
/** 光标闪烁周期。 */
const CARET_BLINK_MS = 500;

interface TypewriterProps {
  /** 已收到的整段文本。它会**增长**（后端一块块推），也可能整体被替换。 */
  text: string;
  /** 文本为空时是否仍显示光标（表示"正在想，还没出内容"）。 */
  showCaretWhenEmpty?: boolean;
  style?: CSSProperties;
}

/**
 * 逐字揭示一段文本。
 *
 * **这不是真流式**：CLI 只给块级输出（实测没有逐 token 的开关），所以数据是
 * 一段段到的，这里只把"揭示速度"做成动画。文本增长时从当前位置接着走，
 * 不会重头再来，也不会等上一段补完才动。
 */
export default function Typewriter({ text, showCaretWhenEmpty, style }: TypewriterProps) {
  const [revealed, setRevealed] = useState(0);
  const revealedRef = useRef(0);
  const [blink, setBlink] = useState(true);

  useEffect(() => {
    // 换了另一段更短的文本（同一位置复用了组件）→ 从头揭示。
    if (revealedRef.current > text.length) {
      revealedRef.current = 0;
      setRevealed(0);
    }
    if (revealedRef.current >= text.length) {
      return;
    }

    const perChar = Math.min(MAX_PER_CHAR_MS, MAX_TOTAL_MS / Math.max(1, text.length));
    const from = revealedRef.current;
    const startedAt = Date.now();
    const timer = setInterval(() => {
      const advanced = Math.ceil((Date.now() - startedAt) / perChar);
      const target = Math.min(text.length, from + advanced);
      if (target !== revealedRef.current) {
        revealedRef.current = target;
        setRevealed(target);
      }
      if (target >= text.length) {
        clearInterval(timer);
      }
    }, 16);

    return () => clearInterval(timer);
  }, [text]);

  const typing = revealed < text.length;
  /** 光标在「正在敲字」或「还没出内容但已开始」时显示。 */
  const caret = typing || (showCaretWhenEmpty === true && text.length === 0);

  useEffect(() => {
    if (!caret) {
      setBlink(false);
      return;
    }
    setBlink(true);
    const timer = setInterval(() => setBlink((value) => !value), CARET_BLINK_MS);
    return () => clearInterval(timer);
  }, [caret]);

  return (
    <span style={style}>
      {text.slice(0, revealed)}
      {caret && <span style={{ opacity: blink ? 1 : 0, color: "var(--text-muted)" }}>▍</span>}
    </span>
  );
}