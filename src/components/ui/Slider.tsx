import { Slider as RSlider } from "radix-ui";

type Props = {
  value: number;
  min: number;
  max: number;
  step?: number;
  label: string;
  onChange: (value: number) => void;
  /** When the user lets go (pointer up or key up). */
  onCommit?: (value: number) => void;
  disabled?: boolean;
};

export function Slider({ value, min, max, step = 1, label, onChange, onCommit, disabled }: Props) {
  return (
    <RSlider.Root
      className="slider"
      value={[value]}
      min={min}
      max={max}
      step={step}
      disabled={disabled}
      onValueChange={([v]) => onChange(v)}
      onValueCommit={onCommit && (([v]) => onCommit(v))}
    >
      <RSlider.Track className="slider-track">
        <RSlider.Range className="slider-range" />
      </RSlider.Track>
      <RSlider.Thumb className="slider-thumb" aria-label={label} />
    </RSlider.Root>
  );
}
