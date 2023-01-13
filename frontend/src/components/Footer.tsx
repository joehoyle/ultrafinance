import { Link } from "react-router-dom";

export default function Footer() {
	return <footer className="bg-gray-50 p-8 grid md:grid-cols-4 space-x-6 text-sm text-gray-400">
		<div>
			<img src={`${import.meta.env.BASE_URL}logo.svg`} width="40" className="" />
			<h5 className=" text-gray-400 mt-4">&copy; Ultrafinance 2023</h5>
		</div>
		<div className="col-span-2">Ultrafinance is created by <a className="text-purple-600" href="https://joehoyle.co.uk">Joe Hoyle</a>. Please tweet me <a className="text-purple-600" href="https://twitter.com/joe_hoyle">@joe_hoyle</a> if you have any questions or suggestions. Ultrafinance is provided with no warrenty or guarantees.</div>
		<div className="flex flex-col">
			<Link to="/login">Log In</Link>
			<Link to="/data-compliance">Data Compliance</Link>
			<a href="https://github.com/joehoyle/ultrafinance/">GitHub</a>
		</div>
	</footer>
}
