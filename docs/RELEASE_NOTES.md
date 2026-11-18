# Study Pen 0.2.1

- The visible ruler now blocks new pen ink from crossing its body, including fast freehand strokes.
- Edge guidance accounts for the full brush width so thick strokes stay outside the ruler.
- A stroke crossing the ruler resumes on the other side without connecting through it. Undo removes all parts of that gesture in one step.

Extract the portable ZIP into a writable folder and run `study-pen.exe`. Verified in-app updates preserve existing preferences. Windows 10 and 11, x86-64.
